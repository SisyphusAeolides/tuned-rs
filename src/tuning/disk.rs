use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::{parse_assignment, read_trimmed, resolve_numeric_assignment, resolve_choice};
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
        other => {
            warn!("Unsupported disk option '{other}' for device '{device}'");
            Ok(())
        }
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
    rollback.record_original(
        &rollback_key("sysfs", &path.to_string_lossy()),
        &current,
    )?;
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
    )? else {
        info!("Keeping readahead for '{device}' at '{current}'");
        return Ok(());
    };

    rollback.record_original(
        &rollback_key("sysfs", &path.to_string_lossy()),
        &current,
    )?;
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
    if let Some(list) = devices {
        return Ok(list
            .split([',', ' '])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect());
    }

    let mut devices = Vec::new();
    for entry in fs::read_dir("/sys/block").context("Failed to read /sys/block")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_tunable_block_device(&name) {
            devices.push(name);
        }
    }
    devices.sort_unstable();
    Ok(devices)
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
    Ok(PathBuf::from("/sys/block").join(device).join("queue").join(option))
}
