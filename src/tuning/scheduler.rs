use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Result};
use regex::RegexSet;

use crate::config;
use crate::profile::PluginOptions;
use crate::profile_units::option_value;
use crate::rollback::Rollback;
use crate::tuning::{generic_sysfs, sysctl};

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
}

static RUNTIME: OnceLock<Mutex<Option<RuntimeState>>> = OnceLock::new();

pub fn apply_options(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
    apply_isolation(options)?;
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
            let _ = set_affinity(snapshot.pid, &snapshot.affinity);
        }
    }
}

fn apply_isolation(options: &PluginOptions) -> Result<()> {
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
        let original = match get_affinity(pid) {
            Ok(affinity) => affinity,
            Err(error) if vanished(&error) => continue,
            Err(error) => {
                restore_processes(&state);
                return Err(error);
            }
        };
        if original == desired {
            continue;
        }
        if let Err(error) = set_affinity(pid, &desired) {
            if vanished(&error) {
                continue;
            }
            restore_processes(&state);
            return Err(error);
        }
        state.processes.push(ProcessSnapshot {
            pid,
            identity,
            affinity: original,
        });
    }
    *runtime_slot().lock().unwrap() = Some(state);
    Ok(())
}

fn restore_processes(state: &RuntimeState) {
    for snapshot in state.processes.iter().rev() {
        if process_identity(snapshot.pid).as_deref() == Some(snapshot.identity.as_str()) {
            let _ = set_affinity(snapshot.pid, &snapshot.affinity);
        }
    }
}

fn runtime_slot() -> &'static Mutex<Option<RuntimeState>> {
    RUNTIME.get_or_init(|| Mutex::new(None))
}

fn process_ids() -> Result<Vec<libc::pid_t>> {
    let mut pids = fs::read_dir(config::resolve_path("/proc"))?
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().parse().ok())
        .collect::<Vec<_>>();
    pids.sort_unstable();
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
}
