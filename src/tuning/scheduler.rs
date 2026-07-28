use std::fs;
use std::path::{Path, PathBuf};
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
    pattern: Regex,
}

pub fn apply_options(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
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
    let Some(state) = runtime_slot().lock().unwrap().take() else {
        return;
    };
    for snapshot in state.processes.into_iter().rev() {
        if process_identity(snapshot.pid).as_deref() == Some(snapshot.identity.as_str()) {
            let _ = set_scheduler(snapshot.pid, snapshot.policy, snapshot.priority);
            let _ = set_affinity(snapshot.pid, &snapshot.affinity);
        }
    }
}

fn apply_isolation(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
    let Some(isolated) = option_value(options, "isolated_cores") else {
        return Ok(());
    };
    cleanup();
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
    apply_irq_isolation(rollback, options, &housekeeping)?;
    let process_whitelist = regex_set(option_value(options, "ps_whitelist"))?;
    let process_blacklist = regex_set(option_value(options, "ps_blacklist"))?;
    let cgroup_blacklist = regex_set(option_value(options, "cgroup_ps_blacklist"))?;
    let process_kthreads = option_value(options, "kthread_process")
        .map(parse_bool)
        .transpose()?
        .unwrap_or(true);
    let mut state = RuntimeState::default();
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
        if original.affinity == desired {
            continue;
        }
        if let Err(error) = set_affinity(pid, &desired) {
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
        let policy = rule.policy.unwrap_or(snapshot.policy);
        let priority = rule.priority.unwrap_or_else(|| {
            if rule.policy.is_some() {
                0
            } else {
                snapshot.priority
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
    let affinity = match fields[3].trim() {
        "*" => None,
        value if value.starts_with("cgroup.") => {
            bail!("Scheduler cgroup targets require cgroup initialization")
        }
        value => Some(parse_hex_affinity(value)?),
    };
    Ok(SchedulerRule {
        rule_priority,
        order,
        policy,
        priority,
        affinity,
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
    }
}
