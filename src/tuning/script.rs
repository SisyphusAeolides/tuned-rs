use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use tracing::{debug, info, warn};

use crate::config;
use crate::rollback::{rollback_key, Rollback};

pub fn apply_scripts(rollback: &Rollback, raw: &str) -> Result<()> {
    for script in script_paths(raw) {
        let script = validate_script_path(&script)?;
        rollback.record_original(&rollback_key("script", &script.to_string_lossy()), "stop")?;
        run_script(&script, &["start"])?;
    }
    Ok(())
}

pub fn verify_scripts(raw: &str, ignore_missing: bool) -> Result<bool> {
    let mut verified = true;
    for script in script_paths(raw) {
        let script = match validate_script_path(&script) {
            Ok(script) => script,
            Err(error) if ignore_missing => {
                debug!("Ignoring missing profile script: {error}");
                continue;
            }
            Err(error) => {
                warn!("Profile script verification failed: {error}");
                verified = false;
                continue;
            }
        };
        let mut arguments = vec!["verify"];
        if ignore_missing {
            arguments.push("ignore_missing");
        }
        if let Err(error) = run_script(&script, &arguments) {
            warn!("Profile script verification failed: {error}");
            verified = false;
        }
    }
    Ok(verified)
}

pub fn run_rollback_script(path: &Path, action: &str) -> Result<()> {
    if action != "stop" {
        bail!("Invalid persisted script rollback action '{action}'");
    }
    let script = validate_script_path(path)?;
    run_script(&script, &[action])
}

fn run_script(path: &Path, arguments: &[&str]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Script has no parent directory: {}", path.display()))?;
    info!(
        "Calling profile script '{}' with {arguments:?}",
        path.display()
    );
    let output = Command::new(path)
        .args(arguments)
        .current_dir(parent)
        .output()
        .with_context(|| format!("Failed to execute profile script {}", path.display()))?;

    if !output.stdout.is_empty() {
        debug!(
            "Profile script '{}' output: {}",
            path.display(),
            String::from_utf8_lossy(&output.stdout).trim_end()
        );
    }
    if !output.stderr.is_empty() {
        warn!(
            "Profile script '{}' error output: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim_end()
        );
    }
    if !output.status.success() {
        bail!(
            "Profile script {} exited with {}",
            path.display(),
            output.status
        );
    }
    Ok(())
}

fn validate_script_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("Profile script path must be absolute: {}", path.display());
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Profile script does not exist: {}", path.display()))?;
    let metadata = fs::metadata(&canonical)
        .with_context(|| format!("Cannot inspect profile script {}", canonical.display()))?;
    if !metadata.is_file() {
        bail!(
            "Profile script is not a regular file: {}",
            canonical.display()
        );
    }
    if metadata.permissions().mode() & 0o111 == 0 {
        bail!("Profile script is not executable: {}", canonical.display());
    }

    let roots = config::profile_dirs_from_env()
        .into_iter()
        .map(config::resolve_path_buf)
        .filter_map(|root| root.canonicalize().ok())
        .collect::<Vec<_>>();
    if !roots.iter().any(|root| canonical.starts_with(root)) {
        bail!(
            "Profile script is outside the configured profile directories: {}",
            canonical.display()
        );
    }
    Ok(canonical)
}

fn script_paths(raw: &str) -> Vec<PathBuf> {
    raw.lines()
        .flat_map(|line| line.split(';'))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_script_order_across_merged_lines() {
        assert_eq!(
            script_paths("/one.sh\n/two.sh;/three.sh"),
            vec![
                PathBuf::from("/one.sh"),
                PathBuf::from("/two.sh"),
                PathBuf::from("/three.sh"),
            ]
        );
    }

    #[test]
    fn rejects_unknown_persisted_actions_before_path_access() {
        let error = run_rollback_script(Path::new("/does/not/exist"), "start").unwrap_err();
        assert!(error.to_string().contains("Invalid persisted"));
    }
}
