use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tracing::info;

use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;

const ALLOWED_ROOTS: &[&str] = &[
    "/sys/devices/system/cpu/",
    "/sys/block/",
    "/sys/firmware/acpi/",
    "/sys/kernel/mm/transparent_hugepage/",
    "/sys/kernel/mm/redhat_transparent_hugepage/",
];

pub fn write_raw(path: &Path, payload: &str) -> Result<()> {
    info!("Writing '{payload}' to {}", path.display());
    fs::write(path, payload).with_context(|| format!("Failed to write to {}", path.display()))?;
    Ok(())
}

pub fn write_with_rollback(
    rollback: &Rollback,
    kind: &str,
    path: &Path,
    payload: &str,
) -> Result<()> {
    let path = allowed_sysfs_path(path)?;
    let original = read_trimmed(&path)?;
    rollback.record_original(&rollback_key(kind, &path.to_string_lossy()), &original)?;
    write_raw(&path, payload)
}

pub fn allowed_sysfs_path(path: &Path) -> Result<PathBuf> {
    let path_str = path.to_string_lossy();
    if !ALLOWED_ROOTS.iter().any(|root| path_str.starts_with(root)) {
        bail!("Refusing write outside allowlisted sysfs roots: {path_str}");
    }
    path.canonicalize()
        .with_context(|| format!("Invalid or inaccessible sysfs path: {}", path.display()))
}
