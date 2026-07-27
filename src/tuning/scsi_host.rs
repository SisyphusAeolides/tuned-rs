use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tracing::{debug, info, warn};

use crate::config;
use crate::profile::PluginOptions;
use crate::profile_units::option_value;
use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;

const ALLOWED_POLICIES: &[&str] = &[
    "min_power",
    "med_power_with_dipm",
    "medium_power",
    "max_performance",
];

pub fn apply_options(rollback: &Rollback, devices: &str, options: &PluginOptions) -> Result<()> {
    let Some(raw_policy) = option_value(options, "alpm") else {
        return Ok(());
    };
    let policies = raw_policy
        .split('|')
        .map(str::trim)
        .filter(|policy| ALLOWED_POLICIES.contains(policy))
        .collect::<Vec<_>>();
    if policies.is_empty() {
        bail!("Invalid SCSI ALPM policy '{raw_policy}'");
    }

    let hosts = resolve_hosts(devices)?;
    let mut updated = 0usize;
    for host in hosts {
        if external_sata_port(&host)? {
            info!(
                "Skipping ALPM for hot-plug capable or external SATA host {}",
                host.display()
            );
            continue;
        }
        let target = host.join("link_power_management_policy");
        if !target.is_file() {
            continue;
        }
        let original = read_trimmed(&target)?;
        let mut applied = false;
        for policy in &policies {
            if original == *policy {
                applied = true;
                break;
            }
            rollback
                .record_original(&rollback_key("sysfs", &target.to_string_lossy()), &original)?;
            match fs::write(&target, policy) {
                Ok(()) => {
                    applied = true;
                    updated += 1;
                    break;
                }
                Err(error) => debug!(
                    "SCSI host {} rejected ALPM policy '{}': {error}",
                    host.display(),
                    policy
                ),
            }
        }
        if !applied {
            warn!(
                "No ALPM fallback from '{raw_policy}' was accepted by {}",
                host.display()
            );
        }
    }
    if updated == 0 {
        debug!("No SCSI host ALPM controls were changed");
    }
    Ok(())
}

pub fn verify_options(devices: &str, options: &PluginOptions, ignore_missing: bool) -> bool {
    let Some(raw_policy) = option_value(options, "alpm") else {
        return true;
    };
    let expected = raw_policy
        .split('|')
        .map(str::trim)
        .filter(|policy| ALLOWED_POLICIES.contains(policy))
        .collect::<Vec<_>>();
    if expected.is_empty() {
        return false;
    }
    let hosts = match resolve_hosts(devices) {
        Ok(hosts) => hosts,
        Err(error) => {
            warn!("Cannot enumerate SCSI hosts: {error}");
            return false;
        }
    };
    if hosts.is_empty() {
        return ignore_missing;
    }

    let mut found = false;
    let mut verified = true;
    for host in hosts {
        let target = host.join("link_power_management_policy");
        if !target.is_file() {
            continue;
        }
        found = true;
        match read_trimmed(&target) {
            Ok(actual) if expected.contains(&actual.as_str()) => {}
            Ok(actual) => {
                warn!(
                    "SCSI ALPM mismatch at {}: expected one of {:?}, actual '{}'",
                    target.display(),
                    expected,
                    actual
                );
                verified = false;
            }
            Err(error) => {
                warn!(
                    "Cannot read SCSI ALPM control {}: {error}",
                    target.display()
                );
                verified = false;
            }
        }
    }
    verified && (found || ignore_missing)
}

fn resolve_hosts(devices: &str) -> Result<Vec<PathBuf>> {
    let base = config::resolve_path("/sys/class/scsi_host");
    if devices.trim() != "*" {
        let mut hosts = Vec::new();
        for device in devices
            .split([',', ' '])
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            validate_host_name(device)?;
            let path = base.join(device);
            if path.exists() {
                hosts.push(path);
            }
        }
        hosts.sort_unstable();
        return Ok(hosts);
    }

    let entries = match fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", base.display()))
        }
    };
    let mut hosts = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            valid_host_name(&name).then(|| entry.path())
        })
        .collect::<Vec<_>>();
    hosts.sort_unstable();
    Ok(hosts)
}

fn external_sata_port(host: &Path) -> Result<bool> {
    let path = host.join("ahci_port_cmd");
    if !path.is_file() {
        return Ok(false);
    }
    let value = read_trimmed(&path)?;
    let value = u32::from_str_radix(value.trim_start_matches("0x"), 16)
        .with_context(|| format!("Invalid AHCI port flags in {}", path.display()))?;
    Ok(value & ((1 << 18) | (1 << 21)) != 0)
}

fn validate_host_name(name: &str) -> Result<()> {
    if valid_host_name(name) {
        Ok(())
    } else {
        bail!("Invalid SCSI host name '{name}'")
    }
}

fn valid_host_name(name: &str) -> bool {
    name.strip_prefix("host")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_kernel_scsi_host_names() {
        assert!(valid_host_name("host0"));
        assert!(valid_host_name("host42"));
        assert!(!valid_host_name("host"));
        assert!(!valid_host_name("../host0"));
    }
}
