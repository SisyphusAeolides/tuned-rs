use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tracing::{debug, info, warn};

use crate::config;
use crate::profile::PluginOptions;
use crate::profile_units::option_value;
use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;

pub fn apply_options(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
    let devices = drm_devices()?;
    if let Some(raw) = option_value(options, "radeon_powersave") {
        apply_radeon_powersave(rollback, &devices, raw)?;
    }
    if let Some(raw) = option_value(options, "panel_power_savings") {
        apply_panel_power_savings(rollback, &devices, raw)?;
    }
    Ok(())
}

pub fn verify_options(options: &PluginOptions, ignore_missing: bool) -> bool {
    let devices = match drm_devices() {
        Ok(devices) => devices,
        Err(error) => {
            warn!("Cannot enumerate DRM devices: {error}");
            return false;
        }
    };
    let mut found = false;
    let mut verified = true;

    if let Some(raw) = option_value(options, "radeon_powersave") {
        let expected = radeon_candidates(raw);
        if expected.is_empty() {
            return false;
        }
        for device in &devices {
            let method = device.join("device/power_method");
            if !method.is_file() {
                continue;
            }
            found = true;
            let actual = match read_trimmed(&method) {
                Ok(method) if method == "profile" => read_trimmed(&device.join("device/power_profile")),
                Ok(method) if method == "dpm" => read_trimmed(&device.join("device/power_dpm_state"))
                    .map(|state| format!("dpm-{state}")),
                Ok(method) => Ok(method),
                Err(error) => Err(error),
            };
            match actual {
                Ok(actual) if expected.iter().any(|candidate| candidate == &actual) => {}
                Ok(actual) => {
                    warn!(
                        "Radeon power policy mismatch for {}: expected {:?}, actual '{}'",
                        device.display(),
                        expected,
                        actual
                    );
                    verified = false;
                }
                Err(error) => {
                    warn!("Cannot read Radeon power policy for {}: {error}", device.display());
                    verified = false;
                }
            }
        }
    }

    if let Some(raw) = option_value(options, "panel_power_savings") {
        let expected = match panel_level(raw) {
            Ok(level) => level.to_string(),
            Err(_) => return false,
        };
        for device in &devices {
            let target = device.join("amdgpu/panel_power_savings");
            if !target.is_file() {
                continue;
            }
            found = true;
            match read_trimmed(&target) {
                Ok(actual) if actual == expected => {}
                Ok(actual) => {
                    warn!(
                        "Panel power-savings mismatch at {}: expected '{}', actual '{}'",
                        target.display(),
                        expected,
                        actual
                    );
                    verified = false;
                }
                Err(error) => {
                    warn!("Cannot read panel power control {}: {error}", target.display());
                    verified = false;
                }
            }
        }
    }

    verified && (found || ignore_missing || options.is_empty())
}

fn apply_radeon_powersave(rollback: &Rollback, devices: &[PathBuf], raw: &str) -> Result<()> {
    let candidates = radeon_candidates(raw);
    if candidates.is_empty() {
        bail!("Invalid radeon_powersave value '{raw}'");
    }
    let mut supported = 0usize;

    for device in devices {
        let method = device.join("device/power_method");
        if !method.is_file() {
            continue;
        }
        supported += 1;
        let mut applied = false;
        for candidate in &candidates {
            let result = match candidate.as_str() {
                "default" | "auto" | "low" | "mid" | "high" => {
                    write_node(rollback, &method, "profile")?;
                    write_node(rollback, &device.join("device/power_profile"), candidate)
                }
                "dynpm" => write_node(rollback, &method, "dynpm"),
                "dpm-battery" | "dpm-balanced" | "dpm-performance" => {
                    write_node(rollback, &method, "dpm")?;
                    write_node(
                        rollback,
                        &device.join("device/power_dpm_state"),
                        candidate.trim_start_matches("dpm-"),
                    )
                }
                _ => continue,
            };
            match result {
                Ok(()) => {
                    applied = true;
                    break;
                }
                Err(error) => debug!(
                    "DRM device {} rejected Radeon policy '{}': {error}",
                    device.display(),
                    candidate
                ),
            }
        }
        if !applied {
            warn!(
                "No radeon_powersave fallback from '{raw}' was accepted by {}",
                device.display()
            );
        }
    }
    if supported == 0 {
        debug!("No Radeon power-method controls were found");
    }
    Ok(())
}

fn apply_panel_power_savings(
    rollback: &Rollback,
    devices: &[PathBuf],
    raw: &str,
) -> Result<()> {
    let level = panel_level(raw)?;
    let mut updated = 0usize;
    for device in devices {
        let target = device.join("amdgpu/panel_power_savings");
        if target.is_file() {
            write_node(rollback, &target, &level.to_string())?;
            updated += 1;
        }
    }
    if updated == 0 {
        debug!("No amdgpu panel_power_savings controls were found");
    } else {
        info!("Updated panel power savings on {updated} DRM connector(s)");
    }
    Ok(())
}

fn drm_devices() -> Result<Vec<PathBuf>> {
    let base = config::resolve_path("/sys/class/drm");
    let entries = match fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("Failed to read {}", base.display())),
    };
    let mut devices = BTreeSet::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("card") || !name.contains('-') {
            continue;
        }
        devices.insert(entry.path());
    }
    Ok(devices.into_iter().collect())
}

fn radeon_candidates(raw: &str) -> Vec<String> {
    raw.split(|character: char| {
        character.is_whitespace() || matches!(character, ',' | ';' | ':')
    })
    .map(str::trim)
    .filter(|candidate| {
        matches!(
            *candidate,
            "default"
                | "auto"
                | "low"
                | "mid"
                | "high"
                | "dynpm"
                | "dpm-battery"
                | "dpm-balanced"
                | "dpm-performance"
        )
    })
    .map(str::to_string)
    .collect()
}

fn panel_level(raw: &str) -> Result<u8> {
    let level = raw
        .trim()
        .parse::<u8>()
        .with_context(|| format!("Invalid panel_power_savings value '{raw}'"))?;
    if level > 4 {
        bail!("panel_power_savings must be in the range 0..=4");
    }
    Ok(level)
}

fn write_node(rollback: &Rollback, path: &Path, value: &str) -> Result<()> {
    if !path.is_file() {
        bail!("DRM control does not exist: {}", path.display());
    }
    let original = read_trimmed(path)?;
    if original == value {
        return Ok(());
    }
    rollback.record_original(&rollback_key("sysfs", &path.to_string_lossy()), &original)?;
    fs::write(path, value).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_radeon_fallbacks_in_profile_order() {
        assert_eq!(
            radeon_candidates("dpm-battery, auto"),
            vec!["dpm-battery", "auto"]
        );
        assert!(radeon_candidates("invalid").is_empty());
    }

    #[test]
    fn validates_panel_power_range() {
        assert_eq!(panel_level("4").unwrap(), 4);
        assert!(panel_level("5").is_err());
    }
}
