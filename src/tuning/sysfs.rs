use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tracing::info;

use crate::config;
use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;

pub fn write_raw(path: &Path, payload: &str) -> Result<()> {
    let path = allowed_sysfs_path(path)?;
    info!("Writing '{payload}' to {}", path.display());
    fs::write(&path, payload).with_context(|| format!("Failed to write to {}", path.display()))?;
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
    canonical_path_below(path, &config::resolve_path("/sys"))
}

fn canonical_path_below(path: &Path, root: &Path) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .with_context(|| format!("Invalid or inaccessible sysfs root: {}", root.display()))?;
    let path = path
        .canonicalize()
        .with_context(|| format!("Invalid or inaccessible sysfs path: {}", path.display()))?;
    if path == root || !path.starts_with(&root) {
        bail!(
            "Refusing write outside resolved sysfs root {}: {}",
            root.display(),
            path.display()
        );
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    #[test]
    fn accepts_controls_below_an_explicit_sysfs_root() {
        let root = TempDir::new().unwrap();
        let sys = root.path().join("sys");
        let control = sys.join("devices/system/cpu/control");
        fs::create_dir_all(control.parent().unwrap()).unwrap();
        fs::write(&control, "1").unwrap();

        assert_eq!(
            canonical_path_below(&control, &sys).unwrap(),
            control.canonicalize().unwrap()
        );
    }

    #[test]
    fn rejects_symlinks_that_escape_the_sysfs_root() {
        let root = TempDir::new().unwrap();
        let sys = root.path().join("sys");
        let outside = root.path().join("outside");
        fs::create_dir_all(&sys).unwrap();
        fs::write(&outside, "secret").unwrap();
        let escape = sys.join("escape");
        symlink(&outside, &escape).unwrap();

        assert!(canonical_path_below(&escape, &sys).is_err());
    }
}
