use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

use crate::config;
use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::{parse_assignment, read_trimmed, resolve_numeric_assignment};

const DEPRECATED_OPTIONS: &[&str] = &["base_reachable_time", "retrans_time"];

pub fn sysctl_path(key: &str) -> Result<PathBuf> {
    if key.is_empty()
        || key.len() > 256
        || key.starts_with('.')
        || key.ends_with('.')
        || key.contains("..")
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        bail!("Invalid sysctl key: {key}");
    }

    let leaf = key.rsplit('.').next().unwrap_or(key);
    if DEPRECATED_OPTIONS.contains(&leaf) {
        bail!("Refusing to set deprecated sysctl option {key}");
    }

    Ok(config::resolve_path(&format!("/proc/sys/{}", key.replace('.', "/"))))
}

pub fn read(key: &str) -> Result<String> {
    read_trimmed(&sysctl_path(key)?)
}

pub fn write_raw(key: &str, value: &str) -> Result<()> {
    validate_value(value)?;
    let path = sysctl_path(key)?;
    info!("Writing '{value}' to {}", path.display());
    std::fs::write(&path, value).with_context(|| format!("Failed to write to {}", path.display()))?;
    Ok(())
}

pub fn apply_option(rollback: &Rollback, key: &str, raw_value: &str) -> Result<()> {
    let assignment = parse_assignment(raw_value);
    let current = match read(key) {
        Ok(current) => current,
        Err(error) => {
            warn!("Skipping sysctl '{key}': {error}");
            return Ok(());
        }
    };

    let Some(resolved) = resolve_numeric_assignment(&assignment, &current)? else {
        info!("Keeping sysctl '{key}' at '{current}'");
        return Ok(());
    };

    rollback.record_original(&rollback_key("sysctl", key), &current)?;
    write_raw(key, &resolved)
}

fn validate_value(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 4096 {
        bail!("Invalid sysctl value");
    }
    if value.chars().any(|c| c == '\n' || c == '\0') {
        bail!("Sysctl value must not contain control characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rollback::Rollback;
    use crate::tuning::modifiers::{parse_assignment, AssignmentOp, resolve_numeric_assignment};
    use tempfile::TempDir;

    #[test]
    fn assignment_operators_match_tuned_semantics() {
        let ge = parse_assignment("=>2048");
        assert_eq!(ge.op, AssignmentOp::GreaterEqual);
        assert_eq!(resolve_numeric_assignment(&ge, "4096").unwrap(), None);
        assert_eq!(
            resolve_numeric_assignment(&ge, "1024").unwrap(),
            Some("2048".to_string())
        );
    }

    #[test]
    fn rollback_records_original_sysctl_value() {
        let root = TempDir::new().unwrap();
        std::env::set_var("TUNED_RS_ROOT", root.path());
        let key_path = root.path().join("proc/sys/vm/swappiness");
        std::fs::create_dir_all(key_path.parent().unwrap()).unwrap();
        std::fs::write(&key_path, "60").unwrap();

        let rollback = Rollback::load().unwrap();
        apply_option(&rollback, "vm.swappiness", "10").unwrap();
        rollback.restore_all().unwrap();

        let restored = std::fs::read_to_string(key_path).unwrap();
        assert_eq!(restored.trim(), "60");
        std::env::remove_var("TUNED_RS_ROOT");
    }

    #[test]
    fn rejects_invalid_sysctl_keys() {
        assert!(sysctl_path("../secret").is_err());
        assert!(sysctl_path("vm.swappiness").is_ok());
    }
}
