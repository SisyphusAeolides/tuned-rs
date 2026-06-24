use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use tracing::{debug, error, info, warn};

use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;
use crate::tuning::sysfs::{allowed_sysfs_path, write_raw as write_sysfs_raw};

const CPUFREQ_BASE: &str = "/sys/devices/system/cpu/cpufreq";

const ALLOWED_GOVERNORS: &[&str] = &[
    "performance",
    "powersave",
    "ondemand",
    "conservative",
    "schedutil",
    "userspace",
];

const ALLOWED_EPP_VALUES: &[&str] = &[
    "default",
    "performance",
    "balance_performance",
    "balance_power",
    "power",
];

pub fn is_allowed_governor(governor: &str) -> bool {
    ALLOWED_GOVERNORS.contains(&governor)
}

pub fn is_allowed_epp(value: &str) -> bool {
    ALLOWED_EPP_VALUES.contains(&value)
}

pub fn apply_governor(rollback: &Rollback, raw: &str) -> Result<()> {
    let available = read_available_values(CPUFREQ_BASE, "scaling_available_governors")?;
    let Some(governor) = resolve_choice_for_available(raw, &available, is_allowed_governor) else {
        warn!("No supported governor found in '{raw}' for available [{available}]");
        return Ok(());
    };
    write_cpu_file(rollback, &governor, scaling_governor_path)
}

pub fn apply_epp(rollback: &Rollback, raw: &str) -> Result<()> {
    let available = read_available_values(CPUFREQ_BASE, "energy_performance_available_preferences")?;
    if available.is_empty() {
        debug!("CPU energy performance preference is not supported on this platform");
        return Ok(());
    }
    let Some(epp) = resolve_choice_for_available(raw, &available, is_allowed_epp) else {
        warn!("No supported EPP value found in '{raw}' for available [{available}]");
        return Ok(());
    };
    write_cpu_file(rollback, &epp, epp_path)
}

fn resolve_choice_for_available(
    raw: &str,
    available: &str,
    is_valid: impl Fn(&str) -> bool,
) -> Option<String> {
    let available: HashSet<&str> = available.split_whitespace().collect();
    for candidate in raw.split('|').map(str::trim).filter(|part| !part.is_empty()) {
        if is_valid(candidate) && available.contains(candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

fn read_available_values(base: &str, leaf: &str) -> Result<String> {
    let base_path = Path::new(base);
    let entries = match fs::read_dir(base_path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error.into()),
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("policy") || !name[6..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let path = entry.path().join(leaf);
        if !path.is_file() {
            continue;
        }

        return read_trimmed(&path);
    }

    Ok(String::new())
}

fn scaling_governor_path(entry: &fs::DirEntry) -> PathBuf {
    entry.path().join("scaling_governor")
}

fn epp_path(entry: &fs::DirEntry) -> PathBuf {
    entry.path().join("energy_performance_preference")
}

fn write_cpu_file(
    rollback: &Rollback,
    value: &str,
    path_for_entry: fn(&fs::DirEntry) -> PathBuf,
) -> Result<()> {
    let updated = write_file_dir(rollback, CPUFREQ_BASE, value, path_for_entry)?;
    if updated == 0 {
        warn!("No CPU tuning nodes were updated");
    } else {
        info!("Updated CPU settings on {updated} node(s)");
    }
    Ok(())
}

fn write_file_dir(
    rollback: &Rollback,
    base: &str,
    value: &str,
    path_for_entry: fn(&fs::DirEntry) -> PathBuf,
) -> Result<usize> {
    let base_path = Path::new(base);
    let entries = match fs::read_dir(base_path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };

    let mut updated = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("policy") || !name[6..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let target = path_for_entry(&entry);
        if !target.exists() {
            continue;
        }

        match write_cpu_node(rollback, &target, value) {
            Ok(()) => updated += 1,
            Err(error) => error!("Failed to write {} for {name}: {error}", target.display()),
        }
    }

    Ok(updated)
}

fn write_cpu_node(rollback: &Rollback, target: &Path, value: &str) -> Result<()> {
    validate_cpu_payload(target, value)?;
    let path = allowed_sysfs_path(target)?;
    let original = read_trimmed(&path)?;
    if original == value {
        debug!("Keeping CPU setting {} at '{value}'", path.display());
        return Ok(());
    }
    rollback.record_original(&rollback_key("sysfs", &path.to_string_lossy()), &original)?;
    write_sysfs_raw(&path, value)
}

fn validate_cpu_payload(path: &Path, payload: &str) -> Result<()> {
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    match leaf {
        "scaling_governor" if !is_allowed_governor(payload) => {
            bail!("Unknown CPU governor: {payload}")
        }
        "energy_performance_preference" if !is_allowed_epp(payload) => {
            bail!("Unknown energy performance preference: {payload}")
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_first_available_governor_from_pipe_list() {
        let chosen = resolve_choice_for_available(
            "schedutil|ondemand|powersave",
            "performance powersave",
            is_allowed_governor,
        );
        assert_eq!(chosen.as_deref(), Some("powersave"));
    }

    #[test]
    fn skips_unavailable_epp_values() {
        let chosen = resolve_choice_for_available(
            "balance_performance|balance_power",
            "default performance balance_performance balance_power power",
            is_allowed_epp,
        );
        assert_eq!(chosen.as_deref(), Some("balance_performance"));
    }

    #[test]
    fn legacy_resolve_choice_still_validates_allowlist() {
        use crate::tuning::modifiers::resolve_choice;

        assert_eq!(
            resolve_choice("schedutil|ondemand", is_allowed_governor).as_deref(),
            Some("schedutil")
        );
    }
}
