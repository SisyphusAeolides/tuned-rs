use std::fs;
use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use tracing::{debug, error, info, warn};
use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;

const NVME_BASE: &str = "/sys/class/nvme";
const BLOCK_BASE: &str = "/sys/block";

pub fn apply_storage_options(rollback: &Rollback, options: &[(String, String)]) -> Result<()> {
    if options.is_empty() { return Ok(()); }
    let mut updated = 0usize;
    for (key, value) in options {
        match apply_storage_option(rollback, key, value) {
            Ok(true) => updated += 1,
            Ok(false) => {}
            Err(error) => error!("Failed to apply storage option {key}={value}: {error}"),
        }
    }
    if updated > 0 { info!("Applied {updated} storage tuning option(s)"); }
    Ok(())
}

fn apply_storage_option(rollback: &Rollback, key: &str, value: &str) -> Result<bool> {
    match key {
        "nvme_apst" => apply_nvme_apst(rollback, value),
        "ssd_trim" => apply_ssd_trim(rollback, value),
        "io_scheduler" => apply_io_scheduler(rollback, value),
        "nr_requests" => apply_nr_requests(rollback, value),
        _ => { warn!("Unknown storage option: {key}"); Ok(false) }
    }
}

fn apply_nvme_apst(rollback: &Rollback, value: &str) -> Result<bool> {
    let nvme_path = Path::new(NVME_BASE);
    if !nvme_path.exists() { return Ok(false); }
    let mut updated = false;
    for entry in fs::read_dir(nvme_path)? {
        let entry = entry?;
        let apst_path = entry.path().join("power/pm_qos_latency_tolerance_us");
        if !apst_path.exists() { continue; }
        let original = read_trimmed(&apst_path)?;
        if original == value { continue; }
        rollback.record_original(&rollback_key("sysfs", &apst_path.to_string_lossy()), &original)?;
        fs::write(&apst_path, format!("{}\n", value))?;
        updated = true;
    }
    Ok(updated)
}

fn apply_ssd_trim(rollback: &Rollback, value: &str) -> Result<bool> {
    debug!("SSD TRIM scheduling via fstrim.timer (systemd service)");
    Ok(false)
}

fn apply_io_scheduler(rollback: &Rollback, value: &str) -> Result<bool> {
    let block_path = Path::new(BLOCK_BASE);
    if !block_path.exists() { return Ok(false); }
    let mut updated = false;
    for entry in fs::read_dir(block_path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("loop") || name.starts_with("ram") { continue; }
        let sched_path = entry.path().join("queue/scheduler");
        if !sched_path.exists() { continue; }
        let original = read_trimmed(&sched_path)?;
        rollback.record_original(&rollback_key("sysfs", &sched_path.to_string_lossy()), &original)?;
        fs::write(&sched_path, value)?;
        updated = true;
    }
    Ok(updated)
}

fn apply_nr_requests(rollback: &Rollback, value: &str) -> Result<bool> {
    let block_path = Path::new(BLOCK_BASE);
    if !block_path.exists() { return Ok(false); }
    let mut updated = false;
    for entry in fs::read_dir(block_path)? {
        let entry = entry?;
        let nr_path = entry.path().join("queue/nr_requests");
        if !nr_path.exists() { continue; }
        let original = read_trimmed(&nr_path)?;
        if original == value { continue; }
        rollback.record_original(&rollback_key("sysfs", &nr_path.to_string_lossy()), &original)?;
        fs::write(&nr_path, format!("{}\n", value))?;
        updated = true;
    }
    Ok(updated)
}
