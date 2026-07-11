use std::fs;
use std::path::Path;
use anyhow::{Context, Result};
use tracing::{debug, error, info, warn};
use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;

const HERMES_MODULE_BASE: &str = "/sys/module/hermes/parameters";
const HERMES_CLASS_BASE: &str = "/sys/class/hermes";

pub fn apply_hermes_options(rollback: &Rollback, options: &[(String, String)]) -> Result<()> {
    if options.is_empty() { return Ok(()); }
    let mut updated = 0usize;
    for (key, value) in options {
        match apply_hermes_option(rollback, key, value) {
            Ok(true) => updated += 1,
            Ok(false) => {}
            Err(error) => error!("Failed to apply Hermes option {key}={value}: {error}"),
        }
    }
    if updated > 0 { info!("Applied {updated} Hermes tuning option(s)"); }
    Ok(())
}

fn apply_hermes_option(rollback: &Rollback, key: &str, value: &str) -> Result<bool> {
    match key {
        "cmd_ring_size" => apply_module_param(rollback, "cmd_ring_size", value),
        "rsp_ring_size" => apply_module_param(rollback, "rsp_ring_size", value),
        "ring_overflow_threshold" => apply_module_param(rollback, "ring_overflow_threshold", value),
        "ring_poll_interval" => apply_module_param(rollback, "ring_poll_interval", value),
        "runtime_pm_enabled" => apply_module_param(rollback, "runtime_pm_enabled", value),
        "idle_timeout_ms" => apply_module_param(rollback, "idle_timeout_ms", value),
        "autosuspend_delay" => apply_module_param(rollback, "autosuspend_delay", value),
        "debug_level" => apply_module_param(rollback, "debug_level", value),
        "error_recovery_mode" => apply_module_param(rollback, "error_recovery_mode", value),
        "firmware_validation" => apply_module_param(rollback, "firmware_validation", value),
        "display_heads" => apply_display_heads(rollback, value),
        "gsp_power_mode" => apply_gsp_power_mode(rollback, value),
        _ => { warn!("Unknown Hermes option: {key}"); Ok(false) }
    }
}

fn apply_module_param(rollback: &Rollback, param: &str, value: &str) -> Result<bool> {
    let param_path = Path::new(HERMES_MODULE_BASE).join(param);
    if !param_path.exists() {
        debug!("Hermes module parameter {param} not available");
        return Ok(false);
    }
    
    let original = read_trimmed(&param_path)?;
    if original == value { return Ok(false); }
    
    rollback.record_original(&rollback_key("sysfs", &param_path.to_string_lossy()), &original)?;
    fs::write(&param_path, format!("{}\n", value))?;
    info!("Set Hermes {param} to {value}");
    Ok(true)
}

fn apply_display_heads(rollback: &Rollback, value: &str) -> Result<bool> {
    let hermes_class = Path::new(HERMES_CLASS_BASE);
    if !hermes_class.exists() { return Ok(false); }
    
    let max_heads: usize = value.parse().unwrap_or(4).min(4);
    let mut updated = false;
    
    for entry in fs::read_dir(hermes_class)? {
        let entry = entry?;
        let device_path = entry.path();
        
        for head in 0..4 {
            let head_path = device_path.join(format!("display/head{}/enabled", head));
            if !head_path.exists() { continue; }
            
            let enable_value = if head < max_heads { "1" } else { "0" };
            let original = read_trimmed(&head_path)?;
            if original == enable_value { continue; }
            
            rollback.record_original(&rollback_key("sysfs", &head_path.to_string_lossy()), &original)?;
            fs::write(&head_path, format!("{}\n", enable_value))?;
            updated = true;
        }
    }
    
    if updated {
        info!("Configured Hermes display heads: {max_heads} active");
    }
    Ok(updated)
}

fn apply_gsp_power_mode(rollback: &Rollback, value: &str) -> Result<bool> {
    let hermes_class = Path::new(HERMES_CLASS_BASE);
    if !hermes_class.exists() { return Ok(false); }
    
    let (idle_timeout, pm_enabled) = match value {
        "performance" => ("0", "0"),
        "balanced" => ("5000", "1"),
        "powersave" => ("1000", "1"),
        _ => { warn!("Unknown GSP power mode: {value}"); return Ok(false); }
    };
    
    let mut updated = false;
    updated |= apply_module_param(rollback, "idle_timeout_ms", idle_timeout)?;
    updated |= apply_module_param(rollback, "runtime_pm_enabled", pm_enabled)?;
    
    if updated {
        info!("Set Hermes GSP power mode to {value}");
    }
    Ok(updated)
}

pub fn get_hermes_stats() -> Result<HermesStats> {
    let hermes_class = Path::new(HERMES_CLASS_BASE);
    if !hermes_class.exists() {
        return Ok(HermesStats::default());
    }
    
    let mut stats = HermesStats::default();
    
    for entry in fs::read_dir(hermes_class)?.flatten() {
        let device_path = entry.path();
        
        if let Ok(val) = read_stat(&device_path, "stats/ring_cmd_count") {
            stats.ring_cmd_count = val;
        }
        if let Ok(val) = read_stat(&device_path, "stats/ring_rsp_count") {
            stats.ring_rsp_count = val;
        }
        if let Ok(val) = read_stat(&device_path, "stats/ring_overflows") {
            stats.ring_overflows = val;
        }
        if let Ok(val) = read_stat(&device_path, "stats/gsp_transitions") {
            stats.gsp_transitions = val;
        }
        if let Ok(state) = read_trimmed(&device_path.join("gsp_state")) {
            stats.gsp_state = state;
        }
    }
    
    Ok(stats)
}

fn read_stat(device_path: &Path, stat_name: &str) -> Result<u64> {
    let stat_path = device_path.join(stat_name);
    if !stat_path.exists() { return Ok(0); }
    Ok(read_trimmed(&stat_path)?.parse().unwrap_or(0))
}

#[derive(Debug, Clone, Default)]
pub struct HermesStats {
    pub ring_cmd_count: u64,
    pub ring_rsp_count: u64,
    pub ring_overflows: u64,
    pub gsp_transitions: u64,
    pub gsp_state: String,
}
