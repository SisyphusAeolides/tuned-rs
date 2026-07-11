use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::{info, warn};

use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::{read_trimmed, resolve_choice};
use crate::tuning::sysfs::write_raw as write_sysfs_raw;

const ACPI_DIR: &str = "/sys/firmware/acpi";

pub fn apply_platform_profile(rollback: &Rollback, raw: &str) -> Result<()> {
    let profile_path = platform_profile_path();
    let choices_path = platform_profile_choices_path();
    if !profile_path.is_file() {
        info!("ACPI platform_profile is not supported on this system");
        return Ok(());
    }

    let available: Vec<String> = read_trimmed(&choices_path)?
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let Some(profile) =
        resolve_choice(raw, |candidate| available.iter().any(|item| item == candidate))
    else {
        warn!("Requested platform_profile '{raw}' unavailable");
        return Ok(());
    };

    let current = read_trimmed(&profile_path)?;
    rollback.record_original(
        &rollback_key("sysfs", &profile_path.to_string_lossy()),
        &current,
    )?;
    write_platform_profile(&profile)
}

fn write_platform_profile(profile: &str) -> Result<()> {
    let path = platform_profile_path();
    info!("Setting platform_profile to '{profile}'");
    write_sysfs_raw(&path, profile)
}

fn platform_profile_path() -> PathBuf {
    Path::new(ACPI_DIR).join("platform_profile")
}

fn platform_profile_choices_path() -> PathBuf {
    Path::new(ACPI_DIR).join("platform_profile_choices")
}
