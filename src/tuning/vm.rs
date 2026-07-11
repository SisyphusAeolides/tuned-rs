use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::{parse_assignment, read_trimmed, resolve_numeric_assignment};
use crate::tuning::sysfs::write_raw as write_sysfs_raw;

const VM_OPTIONS: &[&str] = &[
    "dirty_ratio",
    "dirty_background_ratio",
    "dirty_bytes",
    "dirty_background_bytes",
];

const THP_VALUES: &[&str] = &["always", "never", "madvise"];

pub fn write_raw(option: &str, value: &str) -> Result<()> {
    if option == "transparent_hugepages" {
        return write_transparent_hugepages(value);
    }
    let path = vm_path(option)?;
    info!("Writing '{value}' to {}", path.display());
    fs::write(&path, value).with_context(|| format!("Failed to write to {}", path.display()))?;
    Ok(())
}

pub fn apply_options(rollback: &Rollback, options: &[(String, String)]) -> Result<()> {
    for (option, value) in options {
        apply_option(rollback, option, value)?;
    }
    Ok(())
}

pub fn apply_option(rollback: &Rollback, option: &str, raw_value: &str) -> Result<()> {
    match option {
        "transparent_hugepages" | "transparent_hugepage" => {
            apply_transparent_hugepages(rollback, raw_value)
        }
        "transparent_hugepage.defrag" => apply_transparent_hugepage_defrag(rollback, raw_value),
        "dirty_bytes" | "dirty_background_bytes" if raw_value.trim().ends_with('%') => {
            apply_percent_option(rollback, option, raw_value)
        }
        other if VM_OPTIONS.contains(&other) => apply_vm_sysctl(rollback, other, raw_value),
        other => {
            warn!("Unsupported vm option '{other}'");
            Ok(())
        }
    }
}

fn apply_vm_sysctl(rollback: &Rollback, option: &str, raw_value: &str) -> Result<()> {
    let path = match vm_path(option) {
        Ok(path) => path,
        Err(error) => {
            warn!("Skipping vm option '{option}': {error}");
            return Ok(());
        }
    };

    let assignment = parse_assignment(raw_value);
    let current = read_trimmed(&path)?;
    let Some(resolved) = resolve_numeric_assignment(&assignment, &current)? else {
        info!("Keeping vm '{option}' at '{current}'");
        return Ok(());
    };

    rollback.record_original(&rollback_key("vm", option), &current)?;
    write_raw(option, &resolved)
}

fn apply_percent_option(rollback: &Rollback, option: &str, raw_value: &str) -> Result<()> {
    let percent = raw_value
        .trim()
        .trim_end_matches('%')
        .parse::<u64>()
        .with_context(|| format!("Invalid vm percentage value '{raw_value}'"))?;
    let total = total_memory_bytes()?;
    let bytes = total.saturating_mul(percent) / 100;
    apply_vm_sysctl(rollback, option, &bytes.to_string())
}

fn apply_transparent_hugepages(rollback: &Rollback, raw_value: &str) -> Result<()> {
    let value = raw_value.trim();
    if !THP_VALUES.contains(&value) {
        warn!("Unsupported transparent_hugepages value '{value}'");
        return Ok(());
    }
    if fs::read_to_string("/proc/cmdline")?.contains("transparent_hugepage=") {
        info!("transparent_hugepage set in kernel cmdline; skipping profile value");
        return Ok(());
    }
    let path = thp_path()?.join("enabled");
    if !path.is_file() {
        warn!("transparent_hugepages is not supported on this system");
        return Ok(());
    }
    let current = read_trimmed(&path)?;
    rollback.record_original(&rollback_key("vm", "transparent_hugepages"), &current)?;
    write_sysfs_raw(&path, value)
}

fn apply_transparent_hugepage_defrag(rollback: &Rollback, raw_value: &str) -> Result<()> {
    let path = thp_path()?.join("defrag");
    if !path.is_file() {
        warn!("transparent_hugepage.defrag is not supported on this system");
        return Ok(());
    }
    let current = read_trimmed(&path)?;
    rollback.record_original(&rollback_key("vm", "transparent_hugepage.defrag"), &current)?;
    write_sysfs_raw(&path, raw_value.trim())
}

fn write_transparent_hugepages(value: &str) -> Result<()> {
    let path = thp_path()?.join("enabled");
    write_sysfs_raw(&path, value)
}

fn vm_path(option: &str) -> Result<PathBuf> {
    if !VM_OPTIONS.contains(&option) {
        bail!("Unsupported vm option '{option}'");
    }
    Ok(PathBuf::from("/proc/sys/vm").join(option))
}

fn thp_path() -> Result<PathBuf> {
    for path in [
        "/sys/kernel/mm/transparent_hugepage",
        "/sys/kernel/mm/redhat_transparent_hugepage",
    ] {
        if Path::new(path).is_dir() {
            return Ok(PathBuf::from(path));
        }
    }
    bail!("Transparent hugepage interface not found")
}

fn total_memory_bytes() -> Result<u64> {
    let content = fs::read_to_string("/proc/meminfo").context("Failed to read /proc/meminfo")?;
    for line in content.lines() {
        if let Some(kb) = line.strip_prefix("MemTotal:") {
            let kb = kb
                .trim()
                .trim_end_matches(" kB")
                .parse::<u64>()
                .context("Failed to parse MemTotal")?;
            return Ok(kb * 1024);
        }
    }
    bail!("MemTotal not found in /proc/meminfo")
}
