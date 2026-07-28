use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
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
        if matches!(
            name.as_str(),
            "features" | "coalesce" | "pause" | "ring" | "channels"
        ) {
            for device in network_devices(devices)? {
                apply_ethtool(rollback, &device, name, value)?;
            }
        } else if matches!(name.as_str(), "wake_on_lan" | "txqueuelen" | "mtu") {
            for device in network_devices(devices)? {
                apply_link_option(rollback, &device, name, value)?;
            }
        } else if name == "dynamic" {
            parse_bool(value)?;
        } else {
            global.push((name.clone(), value.clone()));
        }
    }
    apply_tcp_options(rollback, &global)
}

#[derive(Debug, Serialize, Deserialize)]
struct EthtoolSnapshot {
    context: String,
    pairs: Vec<(String, String)>,
}

fn apply_ethtool(rollback: &Rollback, device: &str, context: &str, raw: &str) -> Result<()> {
    let requested = context_pairs(context, raw)?;
    let current = match query_context(device, context) {
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
        .into_iter()
        .filter(|(name, _)| supported.iter().any(|(requested, _)| requested == name))
        .collect::<Vec<_>>();
    let snapshot = EthtoolSnapshot {
        context: context.to_string(),
        pairs: original,
    };
    rollback.record_original(
        &crate::rollback::rollback_key("net-ethtool", &format!("{context}/{device}")),
        &serde_json::to_string(&snapshot)?,
    )?;
    set_context(device, context, &supported)
}

pub fn restore_ethtool(target: &str, encoded: &str) -> Result<()> {
    let (context, device) = target
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("Invalid persisted ethtool target"))?;
    let snapshot: EthtoolSnapshot = serde_json::from_str(encoded)?;
    if snapshot.context != context {
        bail!("Persisted ethtool context does not match its rollback key");
    }
    if context == "wake_on_lan" {
        let value = snapshot
            .pairs
            .iter()
            .find(|(name, _)| name == "wol")
            .map(|(_, value)| value.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing Wake-on-LAN rollback value"))?;
        set_wol(device, value)
    } else {
        set_context(device, context, &snapshot.pairs)
    }
}

fn query_context(device: &str, context: &str) -> Result<Vec<(String, String)>> {
    let flag = match context {
        "features" => "-k",
        "coalesce" => "-c",
        "pause" => "-a",
        "ring" => "-g",
        "channels" => "-l",
        _ => bail!("Unsupported ethtool context '{context}'"),
    };
    validate_device(device)?;
    let output = run_ethtool([flag, device])?;
    if !output.status.success() {
        bail!("ethtool {flag} failed for {device} with {}", output.status);
    }
    parse_context(context, &String::from_utf8_lossy(&output.stdout))
}

fn set_context(device: &str, context: &str, pairs: &[(String, String)]) -> Result<()> {
    let flag = match context {
        "features" => "-K",
        "coalesce" => "-C",
        "pause" => "-A",
        "ring" => "-G",
        "channels" => "-L",
        _ => bail!("Unsupported ethtool context '{context}'"),
    };
    validate_device(device)?;
    let mut arguments = vec![flag.to_string(), device.to_string()];
    for (name, value) in pairs {
        arguments.push(name.clone());
        arguments.push(value.clone());
    }
    let output = Command::new("ethtool").args(arguments).output();
    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if output.status.success() || output.status.code() == Some(80) {
        Ok(())
    } else {
        bail!(
            "ethtool {flag} failed for {device}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

fn parse_context(context: &str, raw: &str) -> Result<Vec<(String, String)>> {
    if context == "channels" {
        return parse_current_channels(raw);
    }
    let body = if context == "ring" {
        raw.split("Current hardware settings:")
            .nth(1)
            .unwrap_or(raw)
    } else {
        raw
    };
    Ok(body
        .lines()
        .filter_map(|line| line.trim().split_once(':'))
        .map(|(name, value)| {
            let name = match (context, name.trim()) {
                ("pause", "Autonegotiate") => "autoneg".to_string(),
                (_, name) => name.to_ascii_lowercase().replace(' ', "-"),
            };
            let value = value
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            (name, value)
        })
        .filter(|(name, value)| !name.is_empty() && !value.is_empty())
        .collect())
}

fn context_pairs(context: &str, raw: &str) -> Result<Vec<(String, String)>> {
    if context == "channels" {
        return parameter_pairs(raw);
    }
    let fields = raw.split_whitespace().collect::<Vec<_>>();
    if fields.is_empty() || fields.len() % 2 != 0 {
        bail!("Network {context} settings must be name/value pairs");
    }
    fields
        .chunks_exact(2)
        .map(|pair| {
            if !pair[0]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                || !pair[1]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                bail!("Invalid network {context} parameter");
            }
            Ok((pair[0].to_ascii_lowercase(), pair[1].to_ascii_lowercase()))
        })
        .collect()
}

fn apply_link_option(rollback: &Rollback, device: &str, option: &str, raw: &str) -> Result<()> {
    validate_device(device)?;
    if option == "wake_on_lan" {
        if raw.is_empty()
            || !raw
                .bytes()
                .all(|byte| matches!(byte, b'p' | b'u' | b'm' | b'b' | b'a' | b'g' | b's' | b'd'))
        {
            bail!("Invalid Wake-on-LAN mode");
        }
        let output = run_ethtool([device])?;
        let output_text = String::from_utf8_lossy(&output.stdout);
        let current = output_text
            .lines()
            .find_map(|line| line.trim().strip_prefix("Wake-on: "))
            .ok_or_else(|| anyhow::anyhow!("Cannot read Wake-on-LAN mode for {device}"))?;
        let snapshot = EthtoolSnapshot {
            context: "wake_on_lan".to_string(),
            pairs: vec![("wol".to_string(), current.to_string())],
        };
        rollback.record_original(
            &crate::rollback::rollback_key("net-ethtool", &format!("wake_on_lan/{device}")),
            &serde_json::to_string(&snapshot)?,
        )?;
        return set_wol(device, raw);
    }
    let value = raw.trim().parse::<u32>()?;
    let leaf = if option == "mtu" {
        "mtu"
    } else {
        "tx_queue_len"
    };
    generic_sysfs::apply_options(
        rollback,
        &vec![(format!("/sys/class/net/{device}/{leaf}"), value.to_string())],
    )
}

fn set_wol(device: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Ok(());
    }
    let output = run_ethtool(["-s", device, "wol", value])?;
    if output.status.success() {
        Ok(())
    } else {
        bail!("Failed to set Wake-on-LAN for {device}")
    }
}

fn parse_bool(raw: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "y" | "yes" | "true" | "on" => Ok(true),
        "0" | "n" | "no" | "false" | "off" => Ok(false),
        _ => bail!("Invalid network dynamic boolean '{raw}'"),
    }
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

pub fn verify_device_option(
    devices: &str,
    option: &str,
    expected: &str,
    ignore_missing: bool,
) -> bool {
    let Ok(devices) = network_devices(devices) else {
        return false;
    };
    if devices.is_empty() {
        return ignore_missing;
    }
    devices.into_iter().all(|device| match option {
        "features" | "coalesce" | "pause" | "ring" | "channels" => {
            let Ok(requested) = context_pairs(option, expected) else {
                return false;
            };
            query_context(&device, option).is_ok_and(|current| {
                requested
                    .iter()
                    .all(|pair| current.iter().any(|active| active == pair))
            })
        }
        "wake_on_lan" => run_ethtool([device.as_str()]).is_ok_and(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.trim() == format!("Wake-on: {}", expected.trim()))
        }),
        "mtu" | "txqueuelen" => {
            let leaf = if option == "mtu" {
                "mtu"
            } else {
                "tx_queue_len"
            };
            fs::read_to_string(config::resolve_path(&format!(
                "/sys/class/net/{device}/{leaf}"
            )))
            .is_ok_and(|active| active.trim() == expected.trim())
        }
        "dynamic" => parse_bool(expected).is_ok(),
        _ => false,
    })
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

    #[test]
    fn parses_feature_pause_and_ring_output() {
        let features = parse_context(
            "features",
            "Features for eth0:\nrx-checksumming: on\ntx-checksum-ipv4: off [fixed]\n",
        )
        .unwrap();
        assert!(features.contains(&("rx-checksumming".to_string(), "on".to_string())));
        let pause = parse_context(
            "pause",
            "Pause parameters for eth0:\nAutonegotiate: on\nRX: off\nTX: on\n",
        )
        .unwrap();
        assert!(pause.contains(&("autoneg".to_string(), "on".to_string())));
        let ring = parse_context(
            "ring",
            "Ring parameters for eth0:\nPre-set maximums:\nRX: 4096\nCurrent hardware settings:\nRX: 512\nRX Mini: 0\n",
        )
        .unwrap();
        assert_eq!(ring[0], ("rx".to_string(), "512".to_string()));
    }

    #[test]
    fn rejects_ethtool_parameter_injection() {
        assert!(context_pairs("features", "gro on;reboot").is_err());
        assert!(context_pairs("ring", "rx many").is_ok());
        assert!(parse_bool("sometimes").is_err());
    }
}
