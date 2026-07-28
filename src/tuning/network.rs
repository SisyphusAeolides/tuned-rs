use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use tracing::{debug, info, warn};

use crate::rollback::Rollback;
use crate::tuning::{generic_sysfs, sysctl};
use crate::{config, device_matcher};

const IDLE_LEVEL_STEPS: u8 = 6;

struct Runtime {
    stop: Arc<AtomicBool>,
    worker: JoinHandle<()>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct NetCounters {
    rx_bytes: u64,
    tx_bytes: u64,
}

struct DynamicDevice {
    name: String,
    previous: NetCounters,
    max_rx_delta: u64,
    max_tx_delta: u64,
    idle_rx: u8,
    idle_tx: u8,
    reduced: bool,
}

static RUNTIMES: OnceLock<Mutex<Vec<Runtime>>> = OnceLock::new();

pub fn apply_options(
    rollback: &Rollback,
    devices: &str,
    options: &[(String, String)],
    manage_runtime: bool,
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
    apply_tcp_options(rollback, &global)?;
    if manage_runtime {
        configure_dynamic(rollback, devices, options)?;
    }
    Ok(())
}

fn configure_dynamic(
    rollback: &Rollback,
    selector: &str,
    options: &[(String, String)],
) -> Result<()> {
    let enabled = options
        .iter()
        .rev()
        .find(|(name, _)| name == "dynamic")
        .map(|(_, value)| parse_bool(value))
        .transpose()?
        .unwrap_or(true);
    if !enabled || !config::dynamic_tuning() || std::env::var_os("TUNED_RS_ROOT").is_some() {
        return Ok(());
    }
    let mut devices = Vec::new();
    for name in network_devices(selector)? {
        let Some(previous) = read_net_counters(&name) else {
            continue;
        };
        if !supports_autonegotiation(&name) {
            continue;
        }
        rollback.record_original(
            &crate::rollback::rollback_key("net-advertise", &name),
            "0x03f",
        )?;
        devices.push(DynamicDevice {
            name,
            previous,
            max_rx_delta: 1,
            max_tx_delta: 1,
            idle_rx: 0,
            idle_tx: 0,
            reduced: false,
        });
    }
    if devices.is_empty() {
        return Ok(());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let interval = config::update_interval();
    let worker = thread::Builder::new()
        .name("tuned-rs-network".to_string())
        .spawn(move || dynamic_monitor(worker_stop, interval, devices))?;
    runtime_slots()
        .lock()
        .unwrap()
        .push(Runtime { stop, worker });
    Ok(())
}

fn dynamic_monitor(
    stop: Arc<AtomicBool>,
    interval: std::time::Duration,
    mut devices: Vec<DynamicDevice>,
) {
    while !stop.load(Ordering::Acquire) {
        thread::park_timeout(interval);
        if stop.load(Ordering::Acquire) {
            break;
        }
        for device in &mut devices {
            let Some(current) = read_net_counters(&device.name) else {
                continue;
            };
            if let Some(reduced) = update_dynamic_level(device, current) {
                let advertise = if reduced { "0x00f" } else { "0x03f" };
                if let Err(error) = set_advertise(&device.name, advertise) {
                    warn!(
                        "Failed to dynamically tune network link '{}': {error}",
                        device.name
                    );
                }
            }
        }
    }
}

fn update_dynamic_level(device: &mut DynamicDevice, current: NetCounters) -> Option<bool> {
    let rx_delta = current.rx_bytes.saturating_sub(device.previous.rx_bytes);
    let tx_delta = current.tx_bytes.saturating_sub(device.previous.tx_bytes);
    device.previous = current;
    device.max_rx_delta = device.max_rx_delta.max(rx_delta);
    device.max_tx_delta = device.max_tx_delta.max(tx_delta);
    device.idle_rx = if rx_delta.saturating_mul(100) < device.max_rx_delta {
        device.idle_rx.saturating_add(1)
    } else {
        0
    };
    device.idle_tx = if tx_delta.saturating_mul(100) < device.max_tx_delta {
        device.idle_tx.saturating_add(1)
    } else {
        0
    };
    if !device.reduced && device.idle_rx >= IDLE_LEVEL_STEPS && device.idle_tx >= IDLE_LEVEL_STEPS {
        device.reduced = true;
        Some(true)
    } else if device.reduced && (device.idle_rx == 0 || device.idle_tx == 0) {
        device.reduced = false;
        Some(false)
    } else {
        None
    }
}

fn read_net_counters(device: &str) -> Option<NetCounters> {
    let root = config::resolve_path("/sys/class/net")
        .join(device)
        .join("statistics");
    Some(NetCounters {
        rx_bytes: fs::read_to_string(root.join("rx_bytes"))
            .ok()?
            .trim()
            .parse()
            .ok()?,
        tx_bytes: fs::read_to_string(root.join("tx_bytes"))
            .ok()?
            .trim()
            .parse()
            .ok()?,
    })
}

fn supports_autonegotiation(device: &str) -> bool {
    validate_device(device).is_ok()
        && run_ethtool([device]).is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).lines().any(|line| {
                    line.trim()
                        .eq_ignore_ascii_case("Supports auto-negotiation: Yes")
                })
        })
}

fn set_advertise(device: &str, value: &str) -> Result<()> {
    validate_device(device)?;
    let output = match Command::new("ethtool")
        .args(["-s", device, "autoneg", "on", "advertise", value])
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "ethtool advertise failed for {device}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

pub fn restore_advertise(device: &str, value: &str) -> Result<()> {
    if !matches!(value, "0x03f" | "0x00f") {
        bail!("Invalid persisted network advertisement value");
    }
    set_advertise(device, value)
}

pub fn cleanup() {
    for runtime in runtime_slots().lock().unwrap().drain(..) {
        runtime.stop.store(true, Ordering::Release);
        runtime.worker.thread().unpark();
        let _ = runtime.worker.join();
    }
}

fn runtime_slots() -> &'static Mutex<Vec<Runtime>> {
    RUNTIMES.get_or_init(|| Mutex::new(Vec::new()))
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

    #[test]
    fn dynamic_link_reduces_after_idle_and_recovers_on_traffic() {
        let mut device = DynamicDevice {
            name: "eth0".to_string(),
            previous: NetCounters::default(),
            max_rx_delta: 100,
            max_tx_delta: 100,
            idle_rx: 0,
            idle_tx: 0,
            reduced: false,
        };
        for _ in 0..IDLE_LEVEL_STEPS - 1 {
            assert_eq!(
                update_dynamic_level(&mut device, NetCounters::default()),
                None
            );
        }
        assert_eq!(
            update_dynamic_level(&mut device, NetCounters::default()),
            Some(true)
        );
        assert_eq!(
            update_dynamic_level(
                &mut device,
                NetCounters {
                    rx_bytes: 100,
                    tx_bytes: 100,
                }
            ),
            Some(false)
        );
    }
}
