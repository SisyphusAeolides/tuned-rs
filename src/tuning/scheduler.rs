use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::config;
use crate::profile::PluginOptions;
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

pub fn apply_options(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
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
}
