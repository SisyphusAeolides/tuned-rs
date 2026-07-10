use std::fs;
use std::path::Path;
use anyhow::{Context, Result};
use tracing::{debug, error, info, warn};
use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;

const BATTERY_BASE: &str = "/sys/class/power_supply";

pub fn apply_battery_options(rollback: &Rollback, options: &[(String, String)]) -> Result<()> {
    if options.is_empty() { return Ok(()); }
    let mut updated = 0usize;
    for (key, value) in options {
        match apply_battery_option(rollback, key, value) {
            Ok(true) => updated += 1,
            Ok(false) => {}
            Err(error) => error!("Failed to apply battery option {key}={value}: {error}"),
        }
    }
    if updated > 0 { info!("Applied {updated} battery tuning option(s)"); }
    Ok(())
}

fn apply_battery_option(rollback: &Rollback, key: &str, value: &str) -> Result<bool> {
    match key {
        "charge_start_threshold" => apply_charge_threshold(rollback, "charge_control_start_threshold", value),
        "charge_stop_threshold" => apply_charge_threshold(rollback, "charge_control_end_threshold", value),
        "conservation_mode" => apply_conservation_mode(rollback, value),
        "battery_care_limit" => apply_battery_care_limit(rollback, value),
        _ => { warn!("Unknown battery option: {key}"); Ok(false) }
    }
}

fn apply_charge_threshold(rollback: &Rollback, threshold_name: &str, value: &str) -> Result<bool> {
    let battery_path = Path::new(BATTERY_BASE);
    if !battery_path.exists() { return Ok(false); }
    
    for entry in fs::read_dir(battery_path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("BAT") { continue; }
        
        let threshold_path = entry.path().join(threshold_name);
        if !threshold_path.exists() { continue; }
        
        let original = read_trimmed(&threshold_path)?;
        if original == value { continue; }
        
        rollback.record_original(&rollback_key("sysfs", &threshold_path.to_string_lossy()), &original)?;
        fs::write(&threshold_path, format!("{}\n", value))?;
        info!("Set battery {} to {}", threshold_name, value);
        return Ok(true);
    }
    Ok(false)
}

fn apply_conservation_mode(rollback: &Rollback, value: &str) -> Result<bool> {
    let conservation_path = Path::new("/sys/bus/platform/drivers/ideapad_acpi/VPC2004:00/conservation_mode");
    if !conservation_path.exists() {
        debug!("Conservation mode not available on this system");
        return Ok(false);
    }
    
    let mode_value = match value {
        "on" | "1" | "true" => "1",
        "off" | "0" | "false" => "0",
        _ => { warn!("Invalid conservation mode value: {value}"); return Ok(false); }
    };
    
    let original = read_trimmed(conservation_path)?;
    if original == mode_value { return Ok(false); }
    
    rollback.record_original(&rollback_key("sysfs", &conservation_path.to_string_lossy()), &original)?;
    fs::write(conservation_path, format!("{}\n", mode_value))?;
    info!("Set conservation mode to {value}");
    Ok(true)
}

fn apply_battery_care_limit(rollback: &Rollback, value: &str) -> Result<bool> {
    let care_limit_path = Path::new("/sys/class/power_supply/BAT0/charge_control_end_threshold");
    if !care_limit_path.exists() {
        return apply_charge_threshold(rollback, "charge_control_end_threshold", value);
    }
    
    let original = read_trimmed(care_limit_path)?;
    if original == value { return Ok(false); }
    
    rollback.record_original(&rollback_key("sysfs", &care_limit_path.to_string_lossy()), &original)?;
    fs::write(care_limit_path, format!("{}\n", value))?;
    info!("Set battery care limit to {}%", value);
    Ok(true)
}

pub fn get_battery_status() -> Result<BatteryStatus> {
    let battery_path = Path::new(BATTERY_BASE).join("BAT0");
    if !battery_path.exists() {
        return Ok(BatteryStatus::default());
    }
    
    let capacity = read_trimmed(&battery_path.join("capacity"))?.parse().unwrap_or(0);
    let status = read_trimmed(&battery_path.join("status"))?;
    let health = read_trimmed(&battery_path.join("health")).unwrap_or_else(|_| "Unknown".to_string());
    
    Ok(BatteryStatus {
        capacity,
        status,
        health,
    })
}

#[derive(Debug, Clone, Default)]
pub struct BatteryStatus {
    pub capacity: u8,
    pub status: String,
    pub health: String,
}
