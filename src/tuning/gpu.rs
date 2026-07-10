use std::fs;
use std::path::Path;
use anyhow::{Context, Result};
use tracing::{debug, error, info, warn};
use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;

const AMD_GPU_BASE: &str = "/sys/class/drm";
const NVIDIA_GPU_BASE: &str = "/proc/driver/nvidia/gpus";

pub fn apply_gpu_options(rollback: &Rollback, options: &[(String, String)]) -> Result<()> {
    if options.is_empty() {
        return Ok(());
    }
    let mut updated = 0usize;
    for (key, value) in options {
        match apply_gpu_option(rollback, key, value) {
            Ok(true) => updated += 1,
            Ok(false) => {}
            Err(error) => error!("Failed to apply GPU option {key}={value}: {error}"),
        }
    }
    if updated > 0 {
        info!("Applied {updated} GPU tuning option(s)");
    }
    Ok(())
}

fn apply_gpu_option(rollback: &Rollback, key: &str, value: &str) -> Result<bool> {
    match key {
        "amd_power_profile" => apply_amd_power_profile(rollback, value),
        "amd_power_dpm_force_performance_level" => apply_amd_performance_level(rollback, value),
        "nvidia_power_limit" => apply_nvidia_power_limit(rollback, value),
        _ => { warn!("Unknown GPU option: {key}"); Ok(false) }
    }
}

fn apply_amd_power_profile(rollback: &Rollback, value: &str) -> Result<bool> {
    let amd_path = Path::new(AMD_GPU_BASE);
    if !amd_path.exists() {
        debug!("AMD GPU not detected");
        return Ok(false);
    }
    for entry in fs::read_dir(amd_path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("card") { continue; }
        let profile_path = entry.path().join("device/power_dpm_force_performance_level");
        if !profile_path.exists() { continue; }
        let original = read_trimmed(&profile_path)?;
        if original == value { continue; }
        rollback.record_original(&rollback_key("sysfs", &profile_path.to_string_lossy()), &original)?;
        fs::write(&profile_path, format!("{}\n", value))?;
        debug!("Set AMD GPU power profile to {value}");
        return Ok(true);
    }
    Ok(false)
}

fn apply_amd_performance_level(rollback: &Rollback, value: &str) -> Result<bool> {
    apply_amd_power_profile(rollback, value)
}

fn apply_nvidia_power_limit(rollback: &Rollback, value: &str) -> Result<bool> {
    debug!("NVIDIA power limit control requires nvidia-smi");
    Ok(false)
}
