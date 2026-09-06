use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::config;
use crate::device_matcher;
use crate::profile::PluginOptions;
use crate::profile_units::option_value;
use crate::rollback::Rollback;
use crate::tuning::generic_sysfs;

pub fn apply_options(rollback: &Rollback, devices: &str, options: &PluginOptions) -> Result<()> {
    let Some(raw) = option_value(options, "autosuspend") else {
        return Ok(());
    };
    let value = bool_value(raw)?;
    for path in controls(devices)? {
        generic_sysfs::apply_options(rollback, &vec![(logical_path(&path), value.to_string())])?;
    }
    Ok(())
}

pub fn verify_options(devices: &str, options: &PluginOptions, ignore_missing: bool) -> bool {
    let Some(raw) = option_value(options, "autosuspend") else {
        return true;
    };
    let Ok(expected) = bool_value(raw) else {
        return false;
    };
    let Ok(paths) = controls(devices) else {
        return false;
    };
    if paths.is_empty() {
        return ignore_missing;
    }
    paths
        .into_iter()
        .all(|path| generic_sysfs::read_active_value(&path).is_ok_and(|actual| actual == expected))
}

fn controls(devices: &str) -> Result<Vec<PathBuf>> {
    let base = config::resolve_path("/sys/bus/usb/devices");
    let entries = match fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", base.display()))
        }
    };
    let names = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let uevent = fs::read_to_string(entry.path().join("uevent")).ok()?;
            uevent
                .lines()
                .any(|line| line == "DEVTYPE=usb_device")
                .then_some(name)
        })
        .collect::<Vec<_>>();
    let mut paths = device_matcher::filter_names(devices, names)
        .into_iter()
        .map(|name| base.join(name).join("power/autosuspend"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    paths.sort_unstable();
    Ok(paths)
}

fn bool_value(raw: &str) -> Result<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "y" | "yes" | "t" | "true" | "on" => Ok("1"),
        "0" | "n" | "no" | "f" | "false" | "off" => Ok("0"),
        _ => bail!("Invalid USB autosuspend boolean '{raw}'"),
    }
}

fn logical_path(path: &std::path::Path) -> String {
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

    #[test]
    fn accepts_tuned_boolean_spellings() {
        assert_eq!(bool_value("true").unwrap(), "1");
        assert_eq!(bool_value("off").unwrap(), "0");
        assert!(bool_value("occasionally").is_err());
    }
}
