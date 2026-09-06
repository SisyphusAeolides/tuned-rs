use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config;
use crate::device_matcher;
use crate::profile::PluginOptions;
use crate::profile_units::option_value;
use crate::rollback::{rollback_key, Rollback};

pub fn apply_options(rollback: &Rollback, devices: &str, options: &PluginOptions) -> Result<()> {
    let Some(raw_affinity) = option_value(options, "affinity") else {
        return Ok(());
    };
    if raw_affinity.trim().is_empty() {
        return Ok(());
    }
    let desired = parse_cpu_list(raw_affinity)?;
    let mode = option_value(options, "mode").unwrap_or("set").trim();
    if !matches!(mode, "set" | "intersect") {
        bail!("IRQ mode must be 'set' or 'intersect'");
    }

    for device in selected_devices(devices)? {
        let path = affinity_path(&device)?;
        let original = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let original_cpus = parse_hex_mask(original.trim())?;
        let target = if mode == "intersect" {
            let intersection = original_cpus
                .intersection(&desired)
                .copied()
                .collect::<BTreeSet<_>>();
            if intersection.is_empty() {
                desired.clone()
            } else {
                intersection
            }
        } else {
            desired.clone()
        };
        if target == original_cpus {
            continue;
        }
        rollback.record_original(&rollback_key("irq-affinity", &device), original.trim())?;
        if let Err(error) = write_path(&path, &format_hex_mask(&target)) {
            if error
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::raw_os_error)
                == Some(libc::EIO)
            {
                continue;
            }
            return Err(error);
        }
    }
    Ok(())
}

pub fn verify_options(devices: &str, options: &PluginOptions, ignore_missing: bool) -> bool {
    let Some(raw) = option_value(options, "affinity") else {
        return true;
    };
    if raw.trim().is_empty() {
        return true;
    }
    let Ok(desired) = parse_cpu_list(raw) else {
        return false;
    };
    let mode = option_value(options, "mode").unwrap_or("set").trim();
    let Ok(selected) = selected_devices(devices) else {
        return false;
    };
    selected.into_iter().all(|device| {
        let Ok(path) = affinity_path(&device) else {
            return false;
        };
        match fs::read_to_string(path) {
            Ok(raw) => parse_hex_mask(raw.trim()).is_ok_and(|current| {
                (mode == "set" && current == desired)
                    || (mode == "intersect" && current.is_subset(&desired))
            }),
            Err(error) => ignore_missing && error.kind() == std::io::ErrorKind::NotFound,
        }
    })
}

pub fn write_raw(device: &str, value: &str) -> Result<()> {
    let path = affinity_path(device)?;
    parse_hex_mask(value)?;
    write_path(&path, value)
}

fn selected_devices(selector: &str) -> Result<Vec<String>> {
    let root = config::resolve_path("/proc/irq");
    let mut names = vec!["DEFAULT".to_string()];
    match fs::read_dir(root) {
        Ok(entries) => {
            names.extend(entries.flatten().filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                (name.bytes().all(|byte| byte.is_ascii_digit()) && entry.path().is_dir())
                    .then(|| format!("irq{name}"))
            }));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(device_matcher::filter_names(selector, names))
}

fn affinity_path(device: &str) -> Result<PathBuf> {
    if device == "DEFAULT" {
        return Ok(config::resolve_path("/proc/irq/default_smp_affinity"));
    }
    let Some(number) = device.strip_prefix("irq") else {
        bail!("Invalid IRQ device '{device}'");
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("Invalid IRQ device '{device}'");
    }
    Ok(config::resolve_path(&format!(
        "/proc/irq/{number}/smp_affinity"
    )))
}

fn write_path(path: &Path, value: &str) -> Result<()> {
    fs::write(path, value).with_context(|| format!("Failed to write {}", path.display()))
}

fn parse_cpu_list(raw: &str) -> Result<BTreeSet<u32>> {
    let mut cpus = BTreeSet::new();
    for field in raw
        .split([',', ' ', '\t'])
        .filter(|field| !field.is_empty())
    {
        if let Some((start, end)) = field.split_once('-') {
            let start = start.parse::<u32>()?;
            let end = end.parse::<u32>()?;
            if start > end || end > 1_048_575 {
                bail!("Invalid CPU range '{field}'");
            }
            cpus.extend(start..=end);
        } else {
            let cpu = field.parse::<u32>()?;
            if cpu > 1_048_575 {
                bail!("CPU identifier is too large");
            }
            cpus.insert(cpu);
        }
    }
    if cpus.is_empty() {
        bail!("IRQ affinity must select at least one CPU");
    }
    Ok(cpus)
}

fn format_hex_mask(cpus: &BTreeSet<u32>) -> String {
    let groups_len = cpus
        .iter()
        .next_back()
        .map_or(1, |cpu| (*cpu as usize / 32) + 1);
    let mut groups = vec![0_u32; groups_len];
    for cpu in cpus {
        groups[*cpu as usize / 32] |= 1_u32 << (*cpu % 32);
    }
    groups
        .iter()
        .rev()
        .enumerate()
        .map(|(index, group)| {
            if index == 0 {
                format!("{group:x}")
            } else {
                format!("{group:08x}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_hex_mask(raw: &str) -> Result<BTreeSet<u32>> {
    let fields = raw.split(',').collect::<Vec<_>>();
    let mut cpus = BTreeSet::new();
    for (group_index, field) in fields.iter().rev().enumerate() {
        if field.is_empty() || field.len() > 8 {
            bail!("Invalid IRQ hexadecimal affinity mask");
        }
        let bits = u32::from_str_radix(field, 16)?;
        for bit in 0..32 {
            if bits & (1 << bit) != 0 {
                cpus.insert((group_index * 32 + bit) as u32);
            }
        }
    }
    Ok(cpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lists_round_trip_through_kernel_hex_masks() {
        let cpus = parse_cpu_list("0,2,31-33,65").unwrap();
        let mask = format_hex_mask(&cpus);
        assert_eq!(mask, "2,00000003,80000005");
        assert_eq!(parse_hex_mask(&mask).unwrap(), cpus);
    }

    #[test]
    fn affinity_paths_reject_device_injection() {
        assert!(affinity_path("irq1/../../cmdline").is_err());
        assert!(affinity_path("DEFAULT").is_ok());
        assert!(affinity_path("irq42").is_ok());
    }
}
