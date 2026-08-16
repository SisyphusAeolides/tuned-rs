use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config;
use crate::device_matcher;
use crate::profile::PluginOptions;
use crate::rollback::Rollback;
use crate::tuning::generic_sysfs;

const UNCORE_ROOT: &str = "/sys/devices/system/cpu/intel_uncore_frequency";

pub fn apply_options(rollback: &Rollback, devices: &str, options: &PluginOptions) -> Result<()> {
    for device in devices_matching(devices)? {
        for (name, raw) in options {
            let bound = match name.as_str() {
                "max_freq_khz" => Bound::Maximum,
                "min_freq_khz" => Bound::Minimum,
                _ => continue,
            };
            let value = resolve_value(&device, bound, raw)?;
            let target = device.join(name);
            generic_sysfs::apply_options(
                rollback,
                &vec![(logical_path(&target), value.to_string())],
            )?;
        }
    }
    Ok(())
}

pub fn verify_options(devices: &str, options: &PluginOptions, ignore_missing: bool) -> bool {
    let Ok(devices) = devices_matching(devices) else {
        return false;
    };
    if devices.is_empty() {
        return ignore_missing;
    }
    devices.into_iter().all(|device| {
        options.iter().all(|(name, raw)| {
            let bound = match name.as_str() {
                "max_freq_khz" => Bound::Maximum,
                "min_freq_khz" => Bound::Minimum,
                _ => return true,
            };
            let Ok(expected) = resolve_value(&device, bound, raw) else {
                return false;
            };
            read_u64(&device.join(name)).is_ok_and(|actual| actual == expected)
        })
    })
}

#[derive(Clone, Copy)]
enum Bound {
    Minimum,
    Maximum,
}

fn resolve_value(device: &Path, bound: Bound, raw: &str) -> Result<u64> {
    let initial_min = read_u64(&device.join("initial_min_freq_khz"))?;
    let initial_max = read_u64(&device.join("initial_max_freq_khz"))?;
    if initial_min > initial_max {
        bail!("Invalid uncore frequency range at {}", device.display());
    }
    let requested = if let Some(percent) = raw.trim().strip_suffix('%') {
        let percent = percent
            .parse::<u64>()
            .with_context(|| format!("Invalid uncore percentage '{raw}'"))?;
        if percent > 100 {
            bail!("Uncore percentage must be between 0 and 100");
        }
        initial_min + percent * (initial_max - initial_min) / 100
    } else {
        raw.trim()
            .parse::<u64>()
            .with_context(|| format!("Invalid uncore frequency '{raw}'"))?
    };
    let current_min = read_u64(&device.join("min_freq_khz"))?;
    let current_max = read_u64(&device.join("max_freq_khz"))?;
    match bound {
        Bound::Maximum if requested < current_min => {
            bail!("Uncore maximum {requested} is below current minimum {current_min}")
        }
        Bound::Minimum if requested > current_max => {
            bail!("Uncore minimum {requested} is above current maximum {current_max}")
        }
        Bound::Maximum => Ok(requested.min(initial_max)),
        Bound::Minimum => Ok(requested.max(initial_min)),
    }
}

fn devices_matching(selector: &str) -> Result<Vec<PathBuf>> {
    let base = config::resolve_path(UNCORE_ROOT);
    let entries = match fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", base.display()))
        }
    };
    let names = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    Ok(device_matcher::filter_names(selector, names)
        .into_iter()
        .map(|name| base.join(name))
        .collect())
}

fn read_u64(path: &Path) -> Result<u64> {
    fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?
        .trim()
        .parse::<u64>()
        .with_context(|| format!("Invalid frequency in {}", path.display()))
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
    fn resolves_percentages_and_caps_hardware_bounds() {
        let root = TempDir::new().unwrap();
        for (name, value) in [
            ("initial_min_freq_khz", "1000"),
            ("initial_max_freq_khz", "5000"),
            ("min_freq_khz", "1000"),
            ("max_freq_khz", "5000"),
        ] {
            fs::write(root.path().join(name), value).unwrap();
        }
        assert_eq!(
            resolve_value(root.path(), Bound::Maximum, "75%").unwrap(),
            4000
        );
        assert_eq!(
            resolve_value(root.path(), Bound::Maximum, "9000").unwrap(),
            5000
        );
        assert_eq!(
            resolve_value(root.path(), Bound::Minimum, "1").unwrap(),
            1000
        );
        assert!(resolve_value(root.path(), Bound::Maximum, "101%").is_err());
    }
}
