use std::fs;
use std::path::Path;
use std::process::Command;
use anyhow::{Context, Result};
use tracing::{debug, error, info, warn};
use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;

const AMD_GPU_BASE: &str = "/sys/class/drm";

pub fn apply_gpu_options(rollback: &Rollback, options: &[(String, String)]) -> Result<()> {
    if options.is_empty() { return Ok(()); }
    let mut updated = 0usize;
    for (key, value) in options {
        match apply_gpu_option(rollback, key, value) {
            Ok(true) => updated += 1,
            Ok(false) => {}
            Err(error) => error!("Failed to apply GPU option {key}={value}: {error}"),
        }
    }
    if updated > 0 { info!("Applied {updated} GPU tuning option(s)"); }
    Ok(())
}

fn apply_gpu_option(rollback: &Rollback, key: &str, value: &str) -> Result<bool> {
    match key {
        "amd_power_profile" => apply_amd_power_profile(rollback, value),
        "amd_power_dpm_force_performance_level" => apply_amd_dpm_level(rollback, value),
        "nvidia_power_limit" => apply_nvidia_power_limit(rollback, value),
        "nvidia_graphics_clock" => apply_nvidia_clock(rollback, "graphics", value),
        "nvidia_memory_clock" => apply_nvidia_clock(rollback, "memory", value),
        "nvidia_persistence_mode" => apply_nvidia_persistence_mode(rollback, value),
        _ => { warn!("Unknown GPU option: {key}"); Ok(false) }
    }
}

fn apply_amd_power_profile(rollback: &Rollback, value: &str) -> Result<bool> {
    let drm_path = Path::new(AMD_GPU_BASE);
    if !drm_path.exists() { return Ok(false); }
    
    for entry in fs::read_dir(drm_path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("card") || name.contains("-") { continue; }
        
        let profile_path = entry.path().join("device/power_dpm_force_performance_level");
        if !profile_path.exists() { continue; }
        
        let original = read_trimmed(&profile_path)?;
        if original == value { continue; }
        
        rollback.record_original(&rollback_key("sysfs", &profile_path.to_string_lossy()), &original)?;
        fs::write(&profile_path, format!("{}\n", value))?;
        info!("Set AMD GPU power profile to {value}");
        return Ok(true);
    }
    Ok(false)
}

fn apply_amd_dpm_level(rollback: &Rollback, value: &str) -> Result<bool> {
    apply_amd_power_profile(rollback, value)
}

fn apply_nvidia_power_limit(_rollback: &Rollback, value: &str) -> Result<bool> {
    let output = Command::new("nvidia-smi")
        .arg("--query-gpu=index")
        .arg("--format=csv,noheader")
        .output();
    
    if output.is_err() {
        debug!("nvidia-smi not available");
        return Ok(false);
    }
    
    let output = output.unwrap();
    let gpu_indices = String::from_utf8_lossy(&output.stdout);
    let mut updated = false;
    
    for index in gpu_indices.lines() {
        let index = index.trim();
        if index.is_empty() { continue; }
        
        let result = Command::new("nvidia-smi")
            .arg("-i")
            .arg(index)
            .arg("-pl")
            .arg(value)
            .status();
        
        if result.is_ok() && result.unwrap().success() {
            info!("Set NVIDIA GPU {index} power limit to {value}W");
            updated = true;
        }
    }
    
    Ok(updated)
}

fn apply_nvidia_clock(_rollback: &Rollback, clock_type: &str, value: &str) -> Result<bool> {
    let output = Command::new("nvidia-smi")
        .arg("--query-gpu=index")
        .arg("--format=csv,noheader")
        .output();
    
    if output.is_err() {
        debug!("nvidia-smi not available");
        return Ok(false);
    }
    
    let output = output.unwrap();
    let gpu_indices = String::from_utf8_lossy(&output.stdout);
    let mut updated = false;
    
    for index in gpu_indices.lines() {
        let index = index.trim();
        if index.is_empty() { continue; }
        
        let arg = match clock_type {
            "graphics" => "-lgc",
            "memory" => "-lmc",
            _ => continue,
        };
        
        let result = Command::new("nvidia-smi")
            .arg("-i")
            .arg(index)
            .arg(arg)
            .arg(value)
            .status();
        
        if result.is_ok() && result.unwrap().success() {
            info!("Set NVIDIA GPU {index} {clock_type} clock to {value} MHz");
            updated = true;
        }
    }
    
    Ok(updated)
}

fn apply_nvidia_persistence_mode(_rollback: &Rollback, value: &str) -> Result<bool> {
    let mode = match value {
        "on" | "1" | "true" => "1",
        "off" | "0" | "false" => "0",
        _ => { warn!("Invalid persistence mode: {value}"); return Ok(false); }
    };
    
    let result = Command::new("nvidia-smi")
        .arg("-pm")
        .arg(mode)
        .status();
    
    if result.is_ok() && result.unwrap().success() {
        info!("Set NVIDIA persistence mode to {value}");
        Ok(true)
    } else {
        Ok(false)
    }
}
