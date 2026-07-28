use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Result};
use regex::{Regex, RegexSet};
use tracing::warn;

use crate::config;
use crate::profile::PluginOptions;
use crate::profile_units::option_value;
use crate::rollback::Rollback;
use crate::tuning::{generic_sysfs, irq, sysctl};

const KNOBS: &[(&str, &str, &str)] = &[
    ("sched_min_granularity_ns", "sched", "min_granularity_ns"),
    ("sched_base_slice_ns", "sched", "min_granularity_ns"),
    ("sched_latency_ns", "sched", "latency_ns"),
    (
        "sched_wakeup_granularity_ns",
        "sched",
        "wakeup_granularity_ns",
    ),
    ("sched_tunable_scaling", "sched", "tunable_scaling"),
    ("sched_migration_cost_ns", "sched", "migration_cost_ns"),
    ("sched_nr_migrate", "sched", "nr_migrate"),
    (
        "numa_balancing_scan_delay_ms",
        "numa_balancing",
        "scan_delay_ms",
    ),
    (
        "numa_balancing_scan_period_min_ms",
        "numa_balancing",
        "scan_period_min_ms",
    ),
    (
        "numa_balancing_scan_period_max_ms",
        "numa_balancing",
        "scan_period_max_ms",
    ),
    (
        "numa_balancing_scan_size_mb",
        "numa_balancing",
        "scan_size_mb",
    ),
];

#[derive(Default)]
struct RuntimeState {
    processes: Vec<ProcessSnapshot>,
    cgroups: Vec<CgroupSnapshot>,
    task_moves: Vec<TaskMove>,
    cgroup_root: Option<PathBuf>,
    mounted_cgroup_root: bool,
    synthetic_cgroup_root: bool,
}

struct CgroupSnapshot {
    path: PathBuf,
    existed: bool,
    cpus: Option<String>,
    mems: Option<String>,
}

struct TaskMove {
    pid: libc::pid_t,
    identity: String,
    original_tasks: PathBuf,
}

struct ProcessSnapshot {
    pid: libc::pid_t,
    identity: String,
    affinity: Vec<u8>,
    policy: i32,
    priority: i32,
}

static RUNTIME: OnceLock<Mutex<Option<RuntimeState>>> = OnceLock::new();

struct SchedulerRule {
    rule_priority: i32,
    order: usize,
    policy: Option<i32>,
    priority: Option<i32>,
    affinity: Option<Vec<u8>>,
    cgroup: Option<String>,
    pattern: Regex,
}

pub fn apply_options(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
    cleanup();
    initialize_cgroups(options)?;
    apply_isolation(rollback, options)?;
    apply_groups(options)?;
    for (name, raw) in options {
        let Some((_, namespace, knob)) = KNOBS.iter().find(|(option, _, _)| option == name) else {
            continue;
        };
        validate_value(raw)?;
        let Some(target) = resolve_knob(namespace, knob) else {
            continue;
        };
        if let Some(key) = proc_sysctl_key(&target) {
            sysctl::apply_option(rollback, &key, raw)?;
        } else {
            generic_sysfs::apply_options(rollback, &vec![(logical_path(&target), raw.clone())])?;
        }
    }
    Ok(())
}

pub fn cleanup() {
    let Some(mut state) = runtime_slot().lock().unwrap().take() else {
        return;
    };
    for moved in state.task_moves.drain(..).rev() {
        if process_identity(moved.pid).as_deref() == Some(moved.identity.as_str()) {
            let _ = fs::write(&moved.original_tasks, moved.pid.to_string());
        }
    }
    for snapshot in state.processes.drain(..).rev() {
        if process_identity(snapshot.pid).as_deref() == Some(snapshot.identity.as_str()) {
            let _ = set_scheduler(snapshot.pid, snapshot.policy, snapshot.priority);
            let _ = set_affinity(snapshot.pid, &snapshot.affinity);
        }
    }
    for snapshot in state.cgroups.drain(..).rev() {
        if snapshot.existed {
            if let Some(cpus) = snapshot.cpus {
                let _ = fs::write(snapshot.path.join("cpuset.cpus"), cpus);
            }
            if let Some(mems) = snapshot.mems {
                let _ = fs::write(snapshot.path.join("cpuset.mems"), mems);
            }
        } else {
            if state.synthetic_cgroup_root {
                let _ = fs::remove_file(snapshot.path.join("tasks"));
                let _ = fs::remove_file(snapshot.path.join("cgroup.procs"));
                let _ = fs::remove_file(snapshot.path.join("cpuset.cpus"));
                let _ = fs::remove_file(snapshot.path.join("cpuset.mems"));
            }
            let _ = fs::remove_dir(&snapshot.path);
        }
    }
    if state.mounted_cgroup_root {
        if let Some(root) = &state.cgroup_root {
            let _ = Command::new("umount").arg(root).status();
        }
    }
}

fn apply_isolation(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
    let Some(isolated) = option_value(options, "isolated_cores") else {
        return Ok(());
    };
    let online = read_cpu_list(&config::resolve_path("/sys/devices/system/cpu/online"))?;
    let isolated = parse_cpu_list(isolated)?;
    let housekeeping = online
        .into_iter()
        .filter(|cpu| isolated.binary_search(cpu).is_err())
        .collect::<Vec<_>>();
    if housekeeping.is_empty() {
        bail!("isolated_cores cannot contain every online CPU");
    }
    let desired = affinity_mask(&housekeeping)?;
    let isolation_cgroup = option_value(options, "cgroup_for_isolated_cores")
        .filter(|value| !value.trim().is_empty())
        .map(sanitize_cgroup_name)
        .transpose()?;
    if let Some(group) = &isolation_cgroup {
        let mut state = runtime_slot().lock().unwrap().take().unwrap_or_default();
        let root = state
            .cgroup_root
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Scheduler cgroup root is not initialized"))?;
        let explicitly_configured = options.iter().any(|(name, _)| {
            name.strip_prefix("cgroup.")
                .and_then(|name| sanitize_cgroup_name(name).ok())
                .is_some_and(|name| &name == group)
        });
        if !explicitly_configured {
            initialize_cgroup(
                &mut state,
                &root,
                group,
                &format_cpu_list(&housekeeping),
                true,
            )?;
        }
        *runtime_slot().lock().unwrap() = Some(state);
    }
    apply_irq_isolation(rollback, options, &housekeeping)?;
    let process_whitelist = regex_set(option_value(options, "ps_whitelist"))?;
    let process_blacklist = regex_set(option_value(options, "ps_blacklist"))?;
    let cgroup_blacklist = regex_set(option_value(options, "cgroup_ps_blacklist"))?;
    let process_kthreads = option_value(options, "kthread_process")
        .map(parse_bool)
        .transpose()?
        .unwrap_or(true);
    let mut state = runtime_slot().lock().unwrap().take().unwrap_or_default();
    let own_pid = std::process::id() as libc::pid_t;
    for pid in process_ids()? {
        if pid == own_pid {
            continue;
        }
        let Some(identity) = process_identity(pid) else {
            continue;
        };
        if process_whitelist
            .as_ref()
            .is_some_and(|set| !set.is_match(&identity))
        {
            continue;
        }
        if !process_kthreads && identity.starts_with('[') && identity.ends_with(']') {
            continue;
        }
        if process_blacklist
            .as_ref()
            .is_some_and(|set| set.is_match(&identity))
        {
            continue;
        }
        if cgroup_blacklist.as_ref().is_some_and(|set| {
            fs::read_to_string(config::resolve_path(&format!("/proc/{pid}/cgroup")))
                .is_ok_and(|cgroup| set.is_match(&cgroup))
        }) {
            continue;
        }
        let original = match snapshot_process(pid, identity) {
            Ok(snapshot) => snapshot,
            Err(error) if vanished(&error) => continue,
            Err(error) => {
                restore_processes(&state);
                return Err(error);
            }
        };
        if isolation_cgroup.is_none() && original.affinity == desired {
            continue;
        }
        let mutation = if let Some(group) = &isolation_cgroup {
            move_task_to_cgroup(&mut state, pid, &original.identity, group)
        } else {
            set_affinity(pid, &desired)
        };
        if let Err(error) = mutation {
            if vanished(&error) {
                continue;
            }
            restore_processes(&state);
            return Err(error);
        }
        state.processes.push(original);
    }
    *runtime_slot().lock().unwrap() = Some(state);
    Ok(())
}

fn apply_irq_isolation(
    rollback: &Rollback,
    options: &PluginOptions,
    housekeeping: &[u32],
) -> Result<()> {
    let process_irqs = option_value(options, "irq_process")
        .map(parse_bool)
        .transpose()?
        .unwrap_or(true);
    let affinity = format_cpu_list(housekeeping);
    if process_irqs {
        irq::apply_options(
            rollback,
            "irq*",
            &vec![
                ("affinity".to_string(), affinity.clone()),
                ("mode".to_string(), "intersect".to_string()),
            ],
        )?;
    }

    match option_value(options, "default_irq_smp_affinity").unwrap_or("calc") {
        "ignore" => Ok(()),
        "calc" => irq::apply_options(
            rollback,
            "DEFAULT",
            &vec![
                ("affinity".to_string(), affinity),
                ("mode".to_string(), "intersect".to_string()),
            ],
        ),
        explicit => irq::apply_options(
            rollback,
            "DEFAULT",
            &vec![
                ("affinity".to_string(), explicit.to_string()),
                ("mode".to_string(), "set".to_string()),
            ],
        ),
    }
}

fn format_cpu_list(cpus: &[u32]) -> String {
    cpus.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn initialize_cgroups(options: &PluginOptions) -> Result<()> {
    let configured = options
        .iter()
        .filter(|(name, value)| name.starts_with("cgroup.") && !value.trim().is_empty())
        .collect::<Vec<_>>();
    let isolated_group =
        option_value(options, "cgroup_for_isolated_cores").filter(|value| !value.trim().is_empty());
    let group_targets = options.iter().any(|(name, value)| {
        name.starts_with("group.")
            && value
                .split(':')
                .nth(3)
                .is_some_and(|target| target.trim().starts_with("cgroup."))
    });
    if configured.is_empty() && isolated_group.is_none() && !group_targets {
        return Ok(());
    }

    let raw_root = option_value(options, "cgroup_mount_point").unwrap_or("/sys/fs/cgroup/cpuset");
    let root = resolve_cgroup_root(raw_root)?;
    let initialize_mount = option_value(options, "cgroup_mount_point_init")
        .map(parse_bool)
        .transpose()?
        .unwrap_or(false);
    let initialize_groups = option_value(options, "cgroup_groups_init")
        .map(parse_bool)
        .transpose()?
        .unwrap_or(true);
    let synthetic = std::env::var_os("TUNED_RS_ROOT").is_some();
    let mut mounted = false;
    if !root.join("cpuset.cpus").is_file() {
        if !initialize_mount {
            bail!(
                "Scheduler cgroup root {} is not an initialized cpuset hierarchy",
                root.display()
            );
        }
        fs::create_dir_all(&root)?;
        if synthetic {
            let online =
                fs::read_to_string(config::resolve_path("/sys/devices/system/cpu/online"))?;
            fs::write(root.join("cpuset.cpus"), online.trim())?;
            fs::write(root.join("cpuset.mems"), "0")?;
            fs::write(root.join("tasks"), "")?;
        } else {
            let status = Command::new("mount")
                .args(["-t", "cgroup", "-o", "cpuset", "cpuset"])
                .arg(&root)
                .status()?;
            if !status.success() {
                bail!("Failed to mount cpuset hierarchy at {}", root.display());
            }
            mounted = true;
        }
    }

    let mut state = RuntimeState {
        cgroup_root: Some(root.clone()),
        mounted_cgroup_root: mounted,
        synthetic_cgroup_root: synthetic,
        ..RuntimeState::default()
    };
    for (name, cpus) in configured {
        let group = sanitize_cgroup_name(name.trim_start_matches("cgroup."))?;
        initialize_cgroup(&mut state, &root, &group, cpus, initialize_groups)?;
    }
    if let Some(group) = isolated_group {
        let group = sanitize_cgroup_name(group)?;
        if !root.join(&group).is_dir() && !initialize_groups {
            bail!("Scheduler cgroup '{}' does not exist", group.display());
        }
    }
    *runtime_slot().lock().unwrap() = Some(state);
    Ok(())
}

fn resolve_cgroup_root(raw: &str) -> Result<PathBuf> {
    let logical = Path::new(raw.trim());
    if !logical.is_absolute()
        || logical
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        || !logical.starts_with("/sys/fs/cgroup")
    {
        bail!("Scheduler cgroup root must remain below /sys/fs/cgroup");
    }
    Ok(config::resolve_path_buf(logical))
}

fn sanitize_cgroup_name(raw: &str) -> Result<PathBuf> {
    let replaced = raw.trim().replace('.', "/");
    if replaced.is_empty()
        || replaced.split('/').any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
    {
        bail!("Invalid scheduler cgroup name '{raw}'");
    }
    Ok(PathBuf::from(replaced))
}

fn initialize_cgroup(
    state: &mut RuntimeState,
    root: &Path,
    group: &Path,
    cpus: &str,
    create: bool,
) -> Result<()> {
    parse_cpu_list(cpus)?;
    let mut relative = PathBuf::new();
    for component in group.components() {
        relative.push(component);
        let path = root.join(&relative);
        let existed = path.is_dir();
        if !existed && !create {
            bail!("Scheduler cgroup {} does not exist", path.display());
        }
        if !existed {
            fs::create_dir(&path)?;
            if state.synthetic_cgroup_root {
                fs::write(path.join("cpuset.cpus"), "")?;
                fs::write(path.join("cpuset.mems"), "")?;
                fs::write(path.join("tasks"), "")?;
            }
        }
        if !state.cgroups.iter().any(|snapshot| snapshot.path == path) {
            state.cgroups.push(CgroupSnapshot {
                cpus: fs::read_to_string(path.join("cpuset.cpus")).ok(),
                mems: fs::read_to_string(path.join("cpuset.mems")).ok(),
                path: path.clone(),
                existed,
            });
        }
        let parent = path.parent().unwrap_or(root);
        let mems = fs::read_to_string(parent.join("cpuset.mems"))?;
        fs::write(path.join("cpuset.mems"), mems.trim())?;
        if fs::read_to_string(path.join("cpuset.cpus"))
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            let parent_cpus = fs::read_to_string(parent.join("cpuset.cpus"))?;
            fs::write(path.join("cpuset.cpus"), parent_cpus.trim())?;
        }
    }
    fs::write(root.join(group).join("cpuset.cpus"), cpus.trim())?;
    Ok(())
}

fn move_task_to_cgroup(
    state: &mut RuntimeState,
    pid: libc::pid_t,
    identity: &str,
    group: impl AsRef<Path>,
) -> Result<()> {
    let root = state
        .cgroup_root
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Scheduler cgroup root is not initialized"))?;
    let target_dir = root.join(group.as_ref());
    if !target_dir.is_dir() {
        bail!("Scheduler cgroup {} does not exist", target_dir.display());
    }
    let target = tasks_file(&target_dir)?;
    if !state.task_moves.iter().any(|moved| moved.pid == pid) {
        state.task_moves.push(TaskMove {
            pid,
            identity: identity.to_string(),
            original_tasks: original_tasks_file(root, pid)?,
        });
    }
    fs::write(target, pid.to_string())?;
    Ok(())
}

fn tasks_file(group: &Path) -> Result<PathBuf> {
    for leaf in ["tasks", "cgroup.procs"] {
        let path = group.join(leaf);
        if path.is_file() {
            return Ok(path);
        }
    }
    bail!("Scheduler cgroup {} has no task control", group.display())
}

fn original_tasks_file(root: &Path, pid: libc::pid_t) -> Result<PathBuf> {
    let contents = fs::read_to_string(config::resolve_path(&format!("/proc/{pid}/cgroup")))?;
    let mut unified = None;
    for line in contents.lines() {
        let fields = line.splitn(3, ':').collect::<Vec<_>>();
        if fields.len() != 3 {
            continue;
        }
        if fields[1]
            .split(',')
            .any(|controller| controller == "cpuset")
        {
            return tasks_file(&root.join(fields[2].trim_start_matches('/')));
        }
        if fields[0] == "0" && fields[1].is_empty() {
            unified = Some(fields[2]);
        }
    }
    if let Some(path) = unified {
        return tasks_file(&root.join(path.trim_start_matches('/')));
    }
    bail!("PID {pid} has no cpuset cgroup membership")
}

fn apply_groups(options: &PluginOptions) -> Result<()> {
    let mut rules = options
        .iter()
        .enumerate()
        .filter(|(_, (name, value))| name.starts_with("group.") && !value.trim().is_empty())
        .map(|(order, (_, value))| parse_group_rule(value, order))
        .collect::<Result<Vec<_>>>()?;
    if rules.is_empty() {
        return Ok(());
    }
    rules.sort_by_key(|rule| (rule.rule_priority, rule.order));
    let process_kthreads = option_value(options, "kthread_process")
        .map(parse_bool)
        .transpose()?
        .unwrap_or(true);
    let mut state = runtime_slot().lock().unwrap().take().unwrap_or_default();
    let own_pid = std::process::id() as libc::pid_t;
    for pid in process_ids()? {
        if pid == own_pid {
            continue;
        }
        let Some(identity) = process_identity(pid) else {
            continue;
        };
        if !process_kthreads && identity.starts_with('[') && identity.ends_with(']') {
            continue;
        }
        let Some(rule) = rules.iter().rfind(|rule| rule.pattern.is_match(&identity)) else {
            continue;
        };
        let existing = state
            .processes
            .iter()
            .position(|snapshot| snapshot.pid == pid);
        let snapshot = if let Some(index) = existing {
            &state.processes[index]
        } else {
            match snapshot_process(pid, identity) {
                Ok(snapshot) => {
                    state.processes.push(snapshot);
                    state.processes.last().expect("snapshot was just inserted")
                }
                Err(error) if vanished(&error) => continue,
                Err(error) => {
                    warn!("Cannot inspect scheduler state for PID {pid}: {error}");
                    continue;
                }
            }
        };
        let snapshot_policy = snapshot.policy;
        let snapshot_priority = snapshot.priority;
        let snapshot_identity = snapshot.identity.clone();
        let policy = rule.policy.unwrap_or(snapshot_policy);
        let priority = rule.priority.unwrap_or_else(|| {
            if rule.policy.is_some() {
                0
            } else {
                snapshot_priority
            }
        });
        if rule.policy.is_some() || rule.priority.is_some() {
            if let Err(error) = set_scheduler(pid, policy, priority) {
                if !vanished(&error) {
                    warn!("Cannot set scheduler policy for PID {pid}: {error}");
                }
            }
        }
        if let Some(affinity) = &rule.affinity {
            if let Err(error) = set_affinity(pid, affinity) {
                if !vanished(&error) {
                    warn!("Cannot set scheduler affinity for PID {pid}: {error}");
                }
            }
        }
        if let Some(group) = &rule.cgroup {
            if let Err(error) = move_task_to_cgroup(&mut state, pid, &snapshot_identity, group) {
                if !vanished(&error) {
                    warn!("Cannot move PID {pid} to scheduler cgroup: {error}");
                }
            }
        }
    }
    *runtime_slot().lock().unwrap() = Some(state);
    Ok(())
}

fn parse_group_rule(raw: &str, order: usize) -> Result<SchedulerRule> {
    let fields = raw.splitn(5, ':').collect::<Vec<_>>();
    if fields.len() != 5 {
        bail!("Scheduler group rule must contain five colon-separated fields");
    }
    let rule_priority = fields[0].trim().parse::<i32>()?;
    let policy = match fields[1].trim() {
        "*" => None,
        "f" => Some(libc::SCHED_FIFO),
        "b" => Some(libc::SCHED_BATCH),
        "r" => Some(libc::SCHED_RR),
        "o" => Some(libc::SCHED_OTHER),
        "i" => Some(libc::SCHED_IDLE),
        value => bail!("Unknown scheduler group policy '{value}'"),
    };
    let priority = match fields[2].trim() {
        "*" => None,
        value => Some(value.parse::<i32>()?),
    };
    let (affinity, cgroup) = match fields[3].trim() {
        "*" => (None, None),
        value if value.starts_with("cgroup.") => (
            None,
            Some(
                sanitize_cgroup_name(value.trim_start_matches("cgroup."))?
                    .to_string_lossy()
                    .into_owned(),
            ),
        ),
        value => (Some(parse_hex_affinity(value)?), None),
    };
    Ok(SchedulerRule {
        rule_priority,
        order,
        policy,
        priority,
        affinity,
        cgroup,
        pattern: Regex::new(fields[4])?,
    })
}

fn parse_hex_affinity(raw: &str) -> Result<Vec<u8>> {
    let raw = raw.strip_prefix("0x").unwrap_or(raw);
    let fields = raw.split(',').collect::<Vec<_>>();
    let mut mask = vec![0u8; libc::CPU_SETSIZE as usize / 8];
    let mut any = false;
    for (group, field) in fields.iter().rev().enumerate() {
        if field.is_empty() || field.len() > 8 {
            bail!("Invalid scheduler hexadecimal affinity '{raw}'");
        }
        let bits = u32::from_str_radix(field, 16)?;
        for bit in 0..32 {
            if bits & (1 << bit) != 0 {
                let cpu = group * 32 + bit;
                if cpu >= mask.len() * 8 {
                    bail!("Scheduler affinity exceeds CPU_SETSIZE");
                }
                mask[cpu / 8] |= 1 << (cpu % 8);
                any = true;
            }
        }
    }
    if !any {
        bail!("Scheduler affinity cannot be empty");
    }
    Ok(mask)
}

fn restore_processes(state: &RuntimeState) {
    for snapshot in state.processes.iter().rev() {
        if process_identity(snapshot.pid).as_deref() == Some(snapshot.identity.as_str()) {
            let _ = set_scheduler(snapshot.pid, snapshot.policy, snapshot.priority);
            let _ = set_affinity(snapshot.pid, &snapshot.affinity);
        }
    }
}

fn runtime_slot() -> &'static Mutex<Option<RuntimeState>> {
    RUNTIME.get_or_init(|| Mutex::new(None))
}

fn process_ids() -> Result<Vec<libc::pid_t>> {
    let mut pids = Vec::new();
    for entry in fs::read_dir(config::resolve_path("/proc"))?.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<libc::pid_t>() else {
            continue;
        };
        match fs::read_dir(entry.path().join("task")) {
            Ok(tasks) => pids.extend(tasks.flatten().filter_map(|task| {
                task.file_name()
                    .to_string_lossy()
                    .parse::<libc::pid_t>()
                    .ok()
            })),
            Err(_) => pids.push(pid),
        }
    }
    pids.sort_unstable();
    pids.dedup();
    Ok(pids)
}

fn process_identity(pid: libc::pid_t) -> Option<String> {
    let root = config::resolve_path(&format!("/proc/{pid}"));
    let cmdline = fs::read(root.join("cmdline")).ok()?;
    if !cmdline.is_empty() {
        return Some(
            String::from_utf8_lossy(&cmdline)
                .replace('\0', " ")
                .trim()
                .to_string(),
        );
    }
    fs::read_to_string(root.join("comm"))
        .ok()
        .map(|name| format!("[{}]", name.trim()))
}

fn snapshot_process(pid: libc::pid_t, identity: String) -> Result<ProcessSnapshot> {
    let affinity = get_affinity(pid)?;
    let policy = unsafe { libc::sched_getscheduler(pid) };
    if policy < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut parameter = libc::sched_param { sched_priority: 0 };
    let result = unsafe { libc::sched_getparam(pid, &mut parameter) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(ProcessSnapshot {
        pid,
        identity,
        affinity,
        policy,
        priority: parameter.sched_priority,
    })
}

fn set_scheduler(pid: libc::pid_t, policy: i32, priority: i32) -> Result<()> {
    let parameter = libc::sched_param {
        sched_priority: priority,
    };
    let result = unsafe { libc::sched_setscheduler(pid, policy, &parameter) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

fn get_affinity(pid: libc::pid_t) -> Result<Vec<u8>> {
    let mut mask = vec![0u8; libc::CPU_SETSIZE as usize / 8];
    // SAFETY: `mask` is writable for the supplied byte length and Linux accepts
    // the cpuset as an opaque byte array through the cpu_set_t pointer type.
    let result = unsafe {
        libc::sched_getaffinity(pid, mask.len(), mask.as_mut_ptr().cast::<libc::cpu_set_t>())
    };
    if result == 0 {
        Ok(mask)
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

fn set_affinity(pid: libc::pid_t, mask: &[u8]) -> Result<()> {
    // SAFETY: `mask` remains readable for the supplied byte length during the
    // syscall and its layout is the Linux affinity bitset ABI.
    let result = unsafe {
        libc::sched_setaffinity(pid, mask.len(), mask.as_ptr().cast::<libc::cpu_set_t>())
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

fn affinity_mask(cpus: &[u32]) -> Result<Vec<u8>> {
    let mut mask = vec![0u8; libc::CPU_SETSIZE as usize / 8];
    for &cpu in cpus {
        let index = cpu as usize;
        if index >= mask.len() * 8 {
            bail!("CPU {cpu} exceeds the supported affinity mask size");
        }
        mask[index / 8] |= 1 << (index % 8);
    }
    Ok(mask)
}

fn read_cpu_list(path: &Path) -> Result<Vec<u32>> {
    parse_cpu_list(fs::read_to_string(path)?.trim())
}

fn parse_cpu_list(raw: &str) -> Result<Vec<u32>> {
    let mut cpus = Vec::new();
    for field in raw
        .split([',', ' ', '\t'])
        .filter(|field| !field.is_empty())
    {
        if let Some((start, end)) = field.split_once('-') {
            let start = start.parse::<u32>()?;
            let end = end.parse::<u32>()?;
            if start > end || end - start > 1_048_576 {
                bail!("Invalid CPU range '{field}'");
            }
            cpus.extend(start..=end);
        } else {
            cpus.push(field.parse::<u32>()?);
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    Ok(cpus)
}

fn regex_set(raw: Option<&str>) -> Result<Option<RegexSet>> {
    let Some(raw) = raw.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    Ok(Some(RegexSet::new(split_regex_list(raw))?))
}

fn split_regex_list(raw: &str) -> Vec<String> {
    let mut patterns = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for character in raw.chars() {
        if escaped {
            if character != ';' {
                current.push('\\');
            }
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == ';' {
            if !current.is_empty() {
                patterns.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        patterns.push(current);
    }
    patterns
}

fn parse_bool(raw: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "y" | "yes" | "t" | "true" | "on" => Ok(true),
        "0" | "n" | "no" | "f" | "false" | "off" => Ok(false),
        _ => bail!("Invalid scheduler boolean value '{raw}'"),
    }
}

fn vanished(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| matches!(error.raw_os_error(), Some(libc::ESRCH) | Some(libc::ENOENT)))
}

pub fn verify_options(options: &PluginOptions, ignore_missing: bool) -> bool {
    options.iter().all(|(name, expected)| {
        let Some((_, namespace, knob)) = KNOBS.iter().find(|(option, _, _)| option == name) else {
            return true;
        };
        if validate_value(expected).is_err() {
            return false;
        }
        let Some(target) = resolve_knob(namespace, knob) else {
            return ignore_missing;
        };
        std::fs::read_to_string(target).is_ok_and(|actual| actual.trim() == expected.trim())
    })
}

fn resolve_knob(namespace: &str, knob: &str) -> Option<PathBuf> {
    resolve_knob_under(
        namespace,
        knob,
        &config::resolve_path("/proc/sys/kernel"),
        &config::resolve_path("/sys/kernel/debug"),
    )
}

fn resolve_knob_under(
    namespace: &str,
    knob: &str,
    proc_root: &Path,
    debug_root: &Path,
) -> Option<PathBuf> {
    let proc = proc_root.join(format!("{namespace}_{knob}"));
    if proc.is_file() {
        return Some(proc);
    }
    let debug = debug_root.join(namespace).join(knob);
    if debug.is_file() {
        return Some(debug);
    }
    if namespace == "sched" && knob == "min_granularity_ns" {
        let renamed = debug_root.join("sched/base_slice_ns");
        if renamed.is_file() {
            return Some(renamed);
        }
    }
    None
}

fn proc_sysctl_key(path: &Path) -> Option<String> {
    let root = config::resolve_path("/proc/sys");
    path.strip_prefix(root).ok().map(|relative| {
        relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join(".")
    })
}

fn validate_value(raw: &str) -> Result<()> {
    if raw.trim().parse::<u64>().is_ok() {
        Ok(())
    } else {
        bail!("Scheduler knob values must be non-negative integers")
    }
}

fn logical_path(path: &Path) -> String {
    if std::env::var_os("TUNED_RS_ROOT").is_some() {
        let root = config::resolve_path("/");
        format!("/{}", path.strip_prefix(root).unwrap_or(path).display())
    } else {
        path.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn follows_kernel_66_scheduler_knob_rename() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("sys/kernel/debug/sched/base_slice_ns");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "3000000").unwrap();
        assert_eq!(
            resolve_knob_under(
                "sched",
                "min_granularity_ns",
                &root.path().join("proc/sys/kernel"),
                &root.path().join("sys/kernel/debug"),
            ),
            Some(path)
        );
    }

    #[test]
    fn rejects_negative_and_symbolic_knob_values() {
        assert!(validate_value("1000").is_ok());
        assert!(validate_value("-1").is_err());
        assert!(validate_value("fast").is_err());
    }

    #[test]
    fn builds_affinity_masks_from_normalized_cpu_lists() {
        assert_eq!(parse_cpu_list("3,1-2,2").unwrap(), vec![1, 2, 3]);
        let mask = affinity_mask(&[0, 2, 9]).unwrap();
        assert_eq!(mask[0], 0b0000_0101);
        assert_eq!(mask[1], 0b0000_0010);
        assert!(parse_cpu_list("4-2").is_err());
    }

    #[test]
    fn scheduler_blacklists_are_semicolon_separated_regexes() {
        let set = regex_set(Some("^\\[ksoftirqd;.*qemu-kvm.*"))
            .unwrap()
            .unwrap();
        assert!(set.is_match("[ksoftirqd/0]"));
        assert!(set.is_match("/usr/bin/qemu-kvm -name guest"));
        assert!(!set.is_match("postgres"));
        let escaped = regex_set(Some(r"literal\;semicolon;postgres"))
            .unwrap()
            .unwrap();
        assert!(escaped.is_match("literal;semicolon"));
        assert!(escaped.is_match("postgres"));
    }

    #[test]
    fn irq_isolation_intersects_affinities_and_rolls_back() {
        let _env_guard = crate::config::test_env_lock();
        let root = TempDir::new().unwrap();
        let irq_root = root.path().join("proc/irq");
        for irq in ["1", "2"] {
            std::fs::create_dir_all(irq_root.join(irq)).unwrap();
        }
        std::fs::write(irq_root.join("1/smp_affinity"), "f").unwrap();
        std::fs::write(irq_root.join("2/smp_affinity"), "2").unwrap();
        std::fs::write(irq_root.join("default_smp_affinity"), "f").unwrap();
        std::env::set_var("TUNED_RS_ROOT", root.path());
        let rollback = Rollback::load().unwrap();

        apply_irq_isolation(&rollback, &Vec::new(), &[0, 2]).unwrap();
        assert_eq!(
            std::fs::read_to_string(irq_root.join("1/smp_affinity")).unwrap(),
            "5"
        );
        assert_eq!(
            std::fs::read_to_string(irq_root.join("2/smp_affinity")).unwrap(),
            "5"
        );
        assert_eq!(
            std::fs::read_to_string(irq_root.join("default_smp_affinity")).unwrap(),
            "5"
        );
        rollback.restore_all().unwrap();
        assert_eq!(
            std::fs::read_to_string(irq_root.join("1/smp_affinity")).unwrap(),
            "f"
        );
        assert_eq!(
            std::fs::read_to_string(irq_root.join("2/smp_affinity")).unwrap(),
            "2"
        );
        assert_eq!(
            std::fs::read_to_string(irq_root.join("default_smp_affinity")).unwrap(),
            "f"
        );
        std::env::remove_var("TUNED_RS_ROOT");
    }

    #[test]
    fn parses_ordered_scheduler_group_rules() {
        let fifo = parse_group_rule(r"10:f:42:5:^worker:[0-9]+$", 3).unwrap();
        assert_eq!(fifo.rule_priority, 10);
        assert_eq!(fifo.order, 3);
        assert_eq!(fifo.policy, Some(libc::SCHED_FIFO));
        assert_eq!(fifo.priority, Some(42));
        assert!(fifo.pattern.is_match("worker:12"));
        let affinity = fifo.affinity.unwrap();
        assert_eq!(affinity[0], 0b0000_0101);

        assert!(parse_group_rule("0:x:0:*:.*", 0).is_err());
        assert!(parse_group_rule("0:o:0:0:.*", 0).is_err());
        assert!(parse_group_rule("missing-fields", 0).is_err());
        let cgroup = parse_group_rule("0:*:*:cgroup.latency.workers:.*", 0).unwrap();
        assert_eq!(cgroup.cgroup.as_deref(), Some("latency/workers"));
        assert!(sanitize_cgroup_name("../escape").is_err());
        assert!(resolve_cgroup_root("/tmp/cgroup").is_err());
    }

    #[test]
    fn initializes_nested_cpuset_groups_and_cleans_them_up() {
        let _env_guard = crate::config::test_env_lock();
        let root = TempDir::new().unwrap();
        let online = root.path().join("sys/devices/system/cpu/online");
        std::fs::create_dir_all(online.parent().unwrap()).unwrap();
        std::fs::write(&online, "0-3").unwrap();
        std::env::set_var("TUNED_RS_ROOT", root.path());
        let options = vec![
            ("cgroup_mount_point_init".to_string(), "true".to_string()),
            ("cgroup.latency.workers".to_string(), "2-3".to_string()),
        ];
        initialize_cgroups(&options).unwrap();
        let cgroup = root.path().join("sys/fs/cgroup/cpuset/latency/workers");
        assert_eq!(
            std::fs::read_to_string(cgroup.join("cpuset.cpus")).unwrap(),
            "2-3"
        );
        assert_eq!(
            std::fs::read_to_string(cgroup.join("cpuset.mems")).unwrap(),
            "0"
        );
        cleanup();
        assert!(!cgroup.exists());
        std::env::remove_var("TUNED_RS_ROOT");
    }
}
