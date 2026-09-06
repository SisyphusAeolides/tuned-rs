use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config;
use crate::device_matcher;
use crate::profile::PluginOptions;
use crate::profile_units::option_value;
use crate::rollback::{rollback_key, Rollback};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mount {
    source: String,
    target: String,
    filesystem: String,
    barriers: bool,
}

pub fn apply_options(rollback: &Rollback, devices: &str, options: &PluginOptions) -> Result<()> {
    let Some(raw) = option_value(options, "disable_barriers") else {
        return Ok(());
    };
    let force = raw.trim().eq_ignore_ascii_case("force");
    if !force && !tuned_bool(raw)? {
        return Ok(());
    }
    for mount in selected_mounts(devices)? {
        if !mount.filesystem.starts_with("ext") || !mount.barriers {
            continue;
        }
        if !force && has_writeback_cache(&mount.source) {
            continue;
        }
        rollback.record_original(
            &rollback_key("mount-barrier", &mount.target),
            if mount.barriers { "1" } else { "0" },
        )?;
        remount(&mount.target, false)?;
    }
    Ok(())
}

pub fn verify_options(devices: &str, options: &PluginOptions, ignore_missing: bool) -> bool {
    let Some(raw) = option_value(options, "disable_barriers") else {
        return true;
    };
    let force = raw.trim().eq_ignore_ascii_case("force");
    let Ok(enabled) = tuned_bool(raw) else {
        return false;
    };
    if !force && !enabled {
        return true;
    }
    match selected_mounts(devices) {
        Ok(mounts) => mounts.into_iter().all(|mount| {
            !mount.filesystem.starts_with("ext")
                || (!force && has_writeback_cache(&mount.source))
                || !mount.barriers
        }),
        Err(_) => ignore_missing,
    }
}

pub fn restore_barrier(target: &str, raw: &str) -> Result<()> {
    validate_mountpoint(target)?;
    let enabled = match raw {
        "0" => false,
        "1" => true,
        _ => bail!("Invalid persisted mount barrier state"),
    };
    remount(target, enabled)
}

fn selected_mounts(selector: &str) -> Result<Vec<Mount>> {
    let contents = fs::read_to_string(config::resolve_path("/proc/mounts"))?;
    let mounts = parse_mounts(&contents)?;
    let selected =
        device_matcher::filter_names(selector, mounts.iter().map(|mount| mount.target.clone()));
    Ok(mounts
        .into_iter()
        .filter(|mount| selected.binary_search(&mount.target).is_ok())
        .collect())
}

fn parse_mounts(contents: &str) -> Result<Vec<Mount>> {
    let mut mounts = Vec::new();
    for (line_number, line) in contents.lines().enumerate() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 4 {
            bail!("Invalid /proc/mounts entry at line {}", line_number + 1);
        }
        if !fields[0].starts_with('/') {
            continue;
        }
        let options = fields[3].split(',').collect::<Vec<_>>();
        let barriers = !options.contains(&"nobarrier") && !options.contains(&"barrier=0");
        mounts.push(Mount {
            source: unescape_mount_field(fields[0])?,
            target: unescape_mount_field(fields[1])?,
            filesystem: fields[2].to_string(),
            barriers,
        });
    }
    Ok(mounts)
}

fn unescape_mount_field(raw: &str) -> Result<String> {
    let bytes = raw.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            if index + 3 >= bytes.len()
                || !bytes[index + 1..=index + 3]
                    .iter()
                    .all(|byte| matches!(byte, b'0'..=b'7'))
            {
                bail!("Invalid mount-field escape");
            }
            let value = (bytes[index + 1] - b'0') * 64
                + (bytes[index + 2] - b'0') * 8
                + (bytes[index + 3] - b'0');
            output.push(value);
            index += 4;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).context("Mount field is not UTF-8")
}

fn validate_mountpoint(target: &str) -> Result<()> {
    let path = Path::new(target);
    if path.is_absolute() && !target.contains('\0') {
        Ok(())
    } else {
        bail!("Invalid mount point '{target}'")
    }
}

fn remount(target: &str, barriers: bool) -> Result<()> {
    validate_mountpoint(target)?;
    if std::env::var_os("TUNED_RS_ROOT").is_some() {
        return Ok(());
    }
    let option = if barriers {
        "remount,barrier=1"
    } else {
        "remount,barrier=0"
    };
    let status = Command::new("/usr/bin/mount")
        .args([target, "-o", option])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        bail!("remounting '{target}' failed with {status}")
    }
}

fn has_writeback_cache(source: &str) -> bool {
    if std::env::var_os("TUNED_RS_ROOT").is_some() {
        return false;
    }
    let Ok(output) = Command::new("lsblk")
        .args(["-sno", "KNAME", source])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|device| {
            let pattern =
                config::resolve_path(&format!("/sys/block/{}/device/scsi_disk", device.trim()));
            fs::read_dir(pattern)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .any(|entry| {
                    fs::read_to_string(entry.path().join("cache_type"))
                        .is_ok_and(|value| value.trim() == "write back")
                })
        })
}

fn tuned_bool(raw: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "y" | "yes" | "t" | "true" | "on" => Ok(true),
        "0" | "n" | "no" | "f" | "false" | "off" => Ok(false),
        _ => bail!("Invalid boolean value '{raw}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_barrier_state_and_escaped_mountpoints() {
        let mounts = parse_mounts(
            "/dev/sda1 / ext4 rw,relatime 0 0\n/dev/sdb1 /srv\\040data ext4 rw,nobarrier 0 0\n",
        )
        .unwrap();
        assert!(mounts[0].barriers);
        assert_eq!(mounts[1].target, "/srv data");
        assert!(!mounts[1].barriers);
    }

    #[test]
    fn rejects_relative_rollback_targets() {
        assert!(validate_mountpoint("../../etc").is_err());
        assert!(validate_mountpoint("/srv/data").is_ok());
    }
}
