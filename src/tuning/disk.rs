use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

use crate::config;
use crate::device_matcher;
use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::{
    parse_assignment, read_trimmed, resolve_choice, resolve_numeric_assignment,
};
use crate::tuning::sysfs::{allowed_sysfs_path, write_raw as write_sysfs_raw};

pub fn apply_options(
    rollback: &Rollback,
    devices: Option<&str>,
    options: &[(String, String)],
) -> Result<()> {
    let devices = resolve_devices(devices)?;
    for device in devices {
        for (option, value) in options {
            apply_device_option(rollback, &device, option, value)?;
        }
    }
    Ok(())
}

pub fn write_raw(device: &str, option: &str, value: &str) -> Result<()> {
    let path = device_option_path(device, option)?;
    write_sysfs_raw(&path, value)
}

fn apply_device_option(
    rollback: &Rollback,
    device: &str,
    option: &str,
    raw_value: &str,
) -> Result<()> {
    match option {
        "elevator" => apply_elevator(rollback, device, raw_value),
        "readahead" => apply_readahead(rollback, device, raw_value),
        "readahead_multiply" => apply_readahead_multiply(rollback, device, raw_value),
        "scheduler_quantum" => apply_scheduler_quantum(rollback, device, raw_value),
        "apm" => apply_hdparm(rollback, device, "apm", "-B", raw_value),
        "spindown" => apply_hdparm(rollback, device, "spindown", "-S", raw_value),
        other => {
            warn!("Unsupported disk option '{other}' for device '{device}'");
            Ok(())
        }
    }
}

fn apply_readahead_multiply(rollback: &Rollback, device: &str, raw_value: &str) -> Result<()> {
    let multiplier = raw_value
        .trim()
        .parse::<f64>()
        .with_context(|| format!("Invalid readahead multiplier '{raw_value}'"))?;
    if !multiplier.is_finite() || multiplier < 0.0 {
        bail!("readahead_multiply must be a finite non-negative number");
    }
    let path = allowed_sysfs_path(&device_option_path(device, "read_ahead_kb")?)?;
    if !path.is_file() {
        return Ok(());
    }
    let current = read_trimmed(&path)?;
    let current_value = current
        .parse::<f64>()
        .with_context(|| format!("Invalid current readahead value for '{device}'"))?;
    let resolved = (current_value * multiplier).trunc();
    if resolved > u64::MAX as f64 {
        bail!("readahead multiplier overflows the kernel control");
    }
    rollback.record_original(&rollback_key("sysfs", &path.to_string_lossy()), &current)?;
    write_sysfs_raw(&path, &(resolved as u64).to_string())
}

fn apply_scheduler_quantum(rollback: &Rollback, device: &str, raw_value: &str) -> Result<()> {
    let value = raw_value
        .trim()
        .parse::<u64>()
        .with_context(|| format!("Invalid scheduler quantum '{raw_value}'"))?;
    let path = allowed_sysfs_path(&device_option_path(device, "iosched/quantum")?)?;
    if !path.is_file() {
        return Ok(());
    }
    let current = read_trimmed(&path)?;
    rollback.record_original(&rollback_key("sysfs", &path.to_string_lossy()), &current)?;
    write_sysfs_raw(&path, &value.to_string())
}

fn apply_hdparm(
    rollback: &Rollback,
    device: &str,
    kind: &str,
    flag: &str,
    raw_value: &str,
) -> Result<()> {
    let value = raw_value
        .trim()
        .parse::<u8>()
        .with_context(|| format!("Invalid disk {kind} value '{raw_value}'"))?;
    let original = if kind == "apm" {
        match query_apm(device)? {
            Some(value) => value,
            None => return Ok(()),
        }
    } else {
        253
    };
    rollback.record_original(
        &rollback_key(&format!("hdparm-{kind}"), device),
        &original.to_string(),
    )?;
    set_hdparm(device, flag, value)
}

fn query_apm(device: &str) -> Result<Option<u8>> {
    let output = match Command::new("hdparm")
        .args(["-B", &format!("/dev/{device}")])
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Failed to execute hdparm"),
    };
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.split('=').nth(1).and_then(|tail| {
        tail.split(|character: char| !character.is_ascii_digit())
            .find(|part| !part.is_empty())
            .and_then(|part| part.parse::<u8>().ok())
    }))
}

fn set_hdparm(device: &str, flag: &str, value: u8) -> Result<()> {
    if !is_tunable_block_device(device)
        || !device
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        bail!("Invalid block device name '{device}'");
    }
    let status = match Command::new("hdparm")
        .args([flag, &value.to_string(), &format!("/dev/{device}")])
        .status()
    {
        Ok(status) => status,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("Failed to execute hdparm"),
    };
    if status.success() {
        Ok(())
    } else {
        bail!("hdparm {flag} failed for /dev/{device} with {status}")
    }
}

pub fn restore_hdparm(kind: &str, device: &str, original: &str) -> Result<()> {
    let value = original
        .parse::<u8>()
        .with_context(|| format!("Invalid saved hdparm value '{original}'"))?;
    match kind {
        "apm" => set_hdparm(device, "-B", value),
        "spindown" => set_hdparm(device, "-S", value),
        _ => bail!("Unknown hdparm rollback kind '{kind}'"),
    }
}

fn apply_elevator(rollback: &Rollback, device: &str, raw_value: &str) -> Result<()> {
    let path = allowed_sysfs_path(&device_option_path(device, "scheduler")?)?;
    if !path.is_file() {
        warn!("Disk elevator is not supported for '{device}'");
        return Ok(());
    }
    let current = read_trimmed(&path)?;
    let resolved = resolve_choice(raw_value, |candidate| current.contains(candidate))
        .unwrap_or_else(|| raw_value.trim().to_string());
    rollback.record_original(&rollback_key("sysfs", &path.to_string_lossy()), &current)?;
    write_sysfs_raw(&path, &resolved)
}

fn apply_readahead(rollback: &Rollback, device: &str, raw_value: &str) -> Result<()> {
    let path = allowed_sysfs_path(&device_option_path(device, "read_ahead_kb")?)?;
    if !path.is_file() {
        warn!("Disk readahead is not supported for '{device}'");
        return Ok(());
    }

    let assignment = parse_assignment(raw_value);
    let target = parse_readahead_kb(&assignment.target)
        .with_context(|| format!("Invalid readahead value '{raw_value}' for '{device}'"))?;
    let current = read_trimmed(&path)?;
    let Some(resolved) = resolve_numeric_assignment(
        &crate::tuning::modifiers::Assignment {
            op: assignment.op,
            raw: assignment.raw.clone(),
            target: target.to_string(),
        },
        &current,
    )?
    else {
        info!("Keeping readahead for '{device}' at '{current}'");
        return Ok(());
    };

    rollback.record_original(&rollback_key("sysfs", &path.to_string_lossy()), &current)?;
    write_sysfs_raw(&path, &resolved)
}

fn parse_readahead_kb(raw: &str) -> Result<i64> {
    let mut parts = raw.split_whitespace();
    let value = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("Missing readahead value"))?
        .parse::<i64>()?;
    if let Some(unit) = parts.next() {
        if unit == "s" {
            return Ok(value / 2);
        }
        bail!("Unsupported readahead unit '{unit}'");
    }
    Ok(value)
}

fn resolve_devices(devices: Option<&str>) -> Result<Vec<String>> {
    let base = config::resolve_path("/sys/block");
    let entries = match fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", base.display()))
        }
    };

    let mut inventory = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_tunable_block_device(&name) {
            inventory.push(name);
        }
    }

    Ok(device_matcher::filter_names(
        devices.unwrap_or("*"),
        inventory,
    ))
}

fn is_tunable_block_device(name: &str) -> bool {
    !(name.starts_with("loop")
        || name.starts_with("ram")
        || name.starts_with("fd")
        || name.starts_with("dm-")
        || name.starts_with("sr"))
}

fn device_option_path(device: &str, option: &str) -> Result<PathBuf> {
    if device.is_empty()
        || device.contains('/')
        || !device
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        bail!("Invalid block device name '{device}'");
    }
    Ok(config::resolve_path("/sys/block")
        .join(device)
        .join("queue")
        .join(option))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_filter_the_supported_inventory() {
        let selected = device_matcher::filter_names(
            "sd* !sda",
            vec!["nvme0n1".to_string(), "sdb".to_string(), "sda".to_string()],
        );
        assert_eq!(selected, ["sdb"]);
    }

    #[test]
    fn rejects_block_device_path_injection() {
        assert!(device_option_path("../sda", "scheduler").is_err());
        assert!(device_option_path("sda/queue", "scheduler").is_err());
    }
}
