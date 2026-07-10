use std::fs;
use std::path::Path;
use anyhow::{Context, Result};
use tracing::{debug, error, info, warn};
use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;

const PROC_NET_BASE: &str = "/proc/sys/net";

pub fn apply_tcp_options(rollback: &Rollback, options: &[(String, String)]) -> Result<()> {
    if options.is_empty() {
        return Ok(());
    }
    let mut updated = 0usize;
    for (key, value) in options {
        match apply_tcp_option(rollback, key, value) {
            Ok(true) => updated += 1,
            Ok(false) => {}
            Err(error) => error!("Failed to apply TCP option {key}={value}: {error}"),
        }
    }
    if updated > 0 {
        info!("Applied {updated} TCP/IP tuning option(s)");
    }
    Ok(())
}

fn apply_tcp_option(rollback: &Rollback, key: &str, value: &str) -> Result<bool> {
    let proc_path = match key {
        "tcp_congestion_control" => format!("{}/ipv4/tcp_congestion_control", PROC_NET_BASE),
        "tcp_window_scaling" => format!("{}/ipv4/tcp_window_scaling", PROC_NET_BASE),
        "tcp_timestamps" => format!("{}/ipv4/tcp_timestamps", PROC_NET_BASE),
        "tcp_sack" => format!("{}/ipv4/tcp_sack", PROC_NET_BASE),
        "tcp_fastopen" => format!("{}/ipv4/tcp_fastopen", PROC_NET_BASE),
        "tcp_rmem" => format!("{}/ipv4/tcp_rmem", PROC_NET_BASE),
        "tcp_wmem" => format!("{}/ipv4/tcp_wmem", PROC_NET_BASE),
        "tcp_max_syn_backlog" => format!("{}/ipv4/tcp_max_syn_backlog", PROC_NET_BASE),
        "tcp_tw_reuse" => format!("{}/ipv4/tcp_tw_reuse", PROC_NET_BASE),
        "tcp_fin_timeout" => format!("{}/ipv4/tcp_fin_timeout", PROC_NET_BASE),
        "core_rmem_max" => format!("{}/core/rmem_max", PROC_NET_BASE),
        "core_wmem_max" => format!("{}/core/wmem_max", PROC_NET_BASE),
        "core_netdev_max_backlog" => format!("{}/core/netdev_max_backlog", PROC_NET_BASE),
        "core_somaxconn" => format!("{}/core/somaxconn", PROC_NET_BASE),
        _ => { warn!("Unknown TCP/IP option: {key}"); return Ok(false); }
    };
    let path = Path::new(&proc_path);
    if !path.exists() { debug!("TCP/IP option {key} not available"); return Ok(false); }
    let original = read_trimmed(path)?;
    if original == value { debug!("TCP/IP option {key} already set"); return Ok(false); }
    rollback.record_original(&rollback_key("proc", &proc_path), &original)?;
    fs::write(path, format!("{}\n", value)).with_context(|| format!("Failed to write {}", path.display()))?;
    debug!("Set TCP/IP option {key} to {value}");
    Ok(true)
}
