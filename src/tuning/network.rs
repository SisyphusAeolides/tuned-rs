use anyhow::Result;
use std::fs;
use std::process::{Command, Output};
use tracing::{debug, info, warn};

use crate::rollback::Rollback;
use crate::tuning::{generic_sysfs, sysctl};
use crate::{config, device_matcher};

pub fn apply_options(
    rollback: &Rollback,
    devices: &str,
    options: &[(String, String)],
) -> Result<()> {
    let mut global = Vec::new();
    for (name, value) in options {
        if name == "channels" {
            for device in network_devices(devices)? {
                apply_channels(rollback, &device, value)?;
            }
        } else {
            global.push((name.clone(), value.clone()));
        }
    }
    apply_tcp_options(rollback, &global)
}

pub fn apply_tcp_options(rollback: &Rollback, options: &[(String, String)]) -> Result<()> {
    if options.is_empty() {
        return Ok(());
    }
    let mut updated = 0usize;
    for (key, value) in options {
        if apply_tcp_option(rollback, key, value)? {
            updated += 1;
        }
    }
    if updated > 0 {
        info!("Applied {updated} TCP/IP tuning option(s)");
    }
    Ok(())
}

pub fn verify_channels(devices: &str, expected: &str, ignore_missing: bool) -> bool {
    let Ok(expected) = parameter_pairs(expected) else {
        return false;
    };
    let Ok(devices) = network_devices(devices) else {
        return false;
    };
    if devices.is_empty() {
        return ignore_missing;
    }
    devices.into_iter().all(|device| {
        query_channels(&device).is_ok_and(|current| {
            expected.iter().all(|(name, value)| {
                current
                    .iter()
                    .any(|pair| pair == &(name.clone(), value.clone()))
            })
        })
    })
}

fn apply_channels(rollback: &Rollback, device: &str, raw: &str) -> Result<()> {
    let requested = parameter_pairs(raw)?;
    let current = match query_channels(device) {
        Ok(current) => current,
        Err(error) if error.to_string().contains("ethtool is not installed") => return Ok(()),
        Err(error) => return Err(error),
    };
    let supported = requested
        .into_iter()
        .filter(|(name, _)| current.iter().any(|(current_name, _)| current_name == name))
        .collect::<Vec<_>>();
    if supported.is_empty() {
        return Ok(());
    }
    let original = current
        .iter()
        .filter(|(name, _)| {
            supported
                .iter()
                .any(|(requested_name, _)| requested_name == name)
        })
        .flat_map(|(name, value)| [name.as_str(), value.as_str()])
        .collect::<Vec<_>>()
        .join(" ");
    rollback.record_original(
        &crate::rollback::rollback_key("net-channels", device),
        &original,
    )?;
    set_channels(device, &supported)
}

pub fn restore_channels(device: &str, original: &str) -> Result<()> {
    set_channels(device, &parameter_pairs(original)?)
}

fn query_channels(device: &str) -> Result<Vec<(String, String)>> {
    validate_device(device)?;
    let output = run_ethtool(["-l", device])?;
    if !output.status.success() {
        anyhow::bail!("ethtool -l failed for {device} with {}", output.status);
    }
    parse_current_channels(&String::from_utf8_lossy(&output.stdout))
}

fn set_channels(device: &str, pairs: &[(String, String)]) -> Result<()> {
    validate_device(device)?;
    let mut arguments = vec!["-L".to_string(), device.to_string()];
    for (name, value) in pairs {
        arguments.push(name.clone());
        arguments.push(value.clone());
    }
    let output = Command::new("ethtool").args(&arguments).output();
    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if output.status.success() || output.status.code() == Some(80) {
        Ok(())
    } else {
        anyhow::bail!(
            "ethtool -L failed for {device}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

fn run_ethtool<'a>(arguments: impl IntoIterator<Item = &'a str>) -> Result<Output> {
    match Command::new("ethtool").args(arguments).output() {
        Ok(output) => Ok(output),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("ethtool is not installed")
        }
        Err(error) => Err(error.into()),
    }
}

fn parse_current_channels(raw: &str) -> Result<Vec<(String, String)>> {
    let current = raw
        .split("Current hardware settings:")
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("ethtool channel output has no current settings"))?;
    Ok(current
        .lines()
        .filter_map(|line| line.trim().split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .filter(|(name, _)| matches!(name.as_str(), "rx" | "tx" | "other" | "combined"))
        .collect())
}

fn parameter_pairs(raw: &str) -> Result<Vec<(String, String)>> {
    let fields = raw.split_whitespace().collect::<Vec<_>>();
    if fields.is_empty() || fields.len() % 2 != 0 {
        anyhow::bail!("Network channel settings must be name/value pairs");
    }
    let mut pairs = Vec::new();
    for pair in fields.chunks_exact(2) {
        let name = pair[0].to_ascii_lowercase();
        if !matches!(name.as_str(), "rx" | "tx" | "other" | "combined") {
            anyhow::bail!("Unsupported network channel parameter '{}'", pair[0]);
        }
        let value = pair[1]
            .parse::<u32>()
            .map_err(|_| anyhow::anyhow!("Invalid network channel count '{}'", pair[1]))?;
        pairs.push((name, value.to_string()));
    }
    Ok(pairs)
}

fn network_devices(selector: &str) -> Result<Vec<String>> {
    let base = config::resolve_path("/sys/class/net");
    let entries = match fs::read_dir(base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let names = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name != "lo")
        .collect::<Vec<_>>();
    Ok(device_matcher::filter_names(selector, names))
}

fn validate_device(device: &str) -> Result<()> {
    if !device.is_empty()
        && device.len() < libc::IFNAMSIZ
        && device.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
    {
        Ok(())
    } else {
        anyhow::bail!("Invalid network device name '{device}'")
    }
}

fn apply_tcp_option(rollback: &Rollback, key: &str, value: &str) -> Result<bool> {
    if key == "nf_conntrack_hashsize" {
        generic_sysfs::apply_options(
            rollback,
            &vec![(
                "/sys/module/nf_conntrack/parameters/hashsize".to_string(),
                value.to_string(),
            )],
        )?;
        return Ok(true);
    }
    let sysctl_key = match key {
        "tcp_congestion_control" => "net.ipv4.tcp_congestion_control",
        "tcp_window_scaling" => "net.ipv4.tcp_window_scaling",
        "tcp_timestamps" => "net.ipv4.tcp_timestamps",
        "tcp_sack" => "net.ipv4.tcp_sack",
        "tcp_fastopen" => "net.ipv4.tcp_fastopen",
        "tcp_rmem" => "net.ipv4.tcp_rmem",
        "tcp_wmem" => "net.ipv4.tcp_wmem",
        "tcp_max_syn_backlog" => "net.ipv4.tcp_max_syn_backlog",
        "tcp_tw_reuse" => "net.ipv4.tcp_tw_reuse",
        "tcp_fin_timeout" => "net.ipv4.tcp_fin_timeout",
        "core_rmem_max" => "net.core.rmem_max",
        "core_wmem_max" => "net.core.wmem_max",
        "core_netdev_max_backlog" => "net.core.netdev_max_backlog",
        "core_somaxconn" => "net.core.somaxconn",
        _ => {
            warn!("Unknown TCP/IP option: {key}");
            return Ok(false);
        }
    };
    sysctl::apply_option(rollback, sysctl_key, value)?;
    debug!("Set TCP/IP option {key} to {value}");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_ethtool_channel_settings() {
        let output = "Channel parameters for eth0:\nPre-set maximums:\nRX: 8\nCombined: 8\nCurrent hardware settings:\nRX: 0\nTX: 0\nOther: 1\nCombined: 4\n";
        assert_eq!(
            parse_current_channels(output).unwrap(),
            vec![
                ("rx".to_string(), "0".to_string()),
                ("tx".to_string(), "0".to_string()),
                ("other".to_string(), "1".to_string()),
                ("combined".to_string(), "4".to_string()),
            ]
        );
    }

    #[test]
    fn validates_channel_parameter_pairs() {
        assert_eq!(
            parameter_pairs("combined 4").unwrap(),
            vec![("combined".to_string(), "4".to_string())]
        );
        assert!(parameter_pairs("combined many").is_err());
        assert!(parameter_pairs("mystery 4").is_err());
    }
}
