use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

    Ok(config::resolve_path(&format!(
        "/proc/sys/{}",
        key.replace('.', "/")
    )))
}

pub fn read(key: &str) -> Result<String> {
    read_trimmed(&sysctl_path(key)?)
}

pub fn write_raw(key: &str, value: &str) -> Result<()> {
    validate_value(value)?;
    let path = sysctl_path(key)?;
    info!("Writing '{value}' to {}", path.display());
    std::fs::write(&path, value)
        .with_context(|| format!("Failed to write to {}", path.display()))?;
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

pub fn reapply_system_configuration(instance: &[(String, String)]) -> Result<()> {
    if !config::reapply_sysctl() || std::env::var_os("TUNED_RS_ROOT").is_some() {
        return Ok(());
    }
    let exclusions = config::reapply_sysctl_exclusions();
    let mut files = HashMap::<String, PathBuf>::new();
    for directory in ["/run/sysctl.d", "/etc/sysctl.d"] {
        let directory = Path::new(directory);
        for entry in std::fs::read_dir(directory).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".conf") {
                files.entry(name).or_insert_with(|| entry.path());
            }
        }
    }
    let mut names = files.keys().cloned().collect::<Vec<_>>();
    names.sort_unstable();
    for name in names {
        apply_system_file(&files[&name], instance, &exclusions)?;
    }
    apply_system_file(Path::new("/etc/sysctl.conf"), instance, &exclusions)
}

fn apply_system_file(
    path: &Path,
    instance: &[(String, String)],
    exclusions: &[String],
) -> Result<()> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", path.display()))
        }
    };
    for (line_number, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warn!(
                "Ignoring malformed sysctl at {}:{}",
                path.display(),
                line_number + 1
            );
            continue;
        };
        let key = key.trim();
        if exclusions
            .iter()
            .any(|pattern| crate::device_matcher::glob_matches(pattern, key))
        {
            continue;
        }
        let value = value.trim();
        if instance.iter().any(|(instance_key, _)| instance_key == key) {
            info!("System sysctl overrides TuneD setting '{key}' with '{value}'");
        }
        if let Err(error) = write_raw(key, value) {
            warn!(
                "Skipping system sysctl '{key}' from {}: {error}",
                path.display()
            );
        }
    }
    Ok(())
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
    use crate::tuning::modifiers::{parse_assignment, resolve_numeric_assignment, AssignmentOp};
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
        let _env_guard = crate::config::test_env_lock();
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

    #[test]
    fn system_configuration_overrides_tuned_except_for_exclusions() {
        let _env_guard = crate::config::test_env_lock();
        let root = TempDir::new().unwrap();
        std::env::set_var("TUNED_RS_ROOT", root.path());
        for key in ["net/ipv4/ip_forward", "vm/swappiness"] {
            let path = root.path().join("proc/sys").join(key);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "1").unwrap();
        }
        let source = root.path().join("override.conf");
        std::fs::write(&source, "net.ipv4.ip_forward = 0\nvm.swappiness = 10\n").unwrap();
        apply_system_file(
            &source,
            &[("net.ipv4.ip_forward".to_string(), "1".to_string())],
            &["net.ipv4.*".to_string()],
        )
        .unwrap();
        assert_eq!(read("net.ipv4.ip_forward").unwrap(), "1");
        assert_eq!(read("vm.swappiness").unwrap(), "10");
        std::env::remove_var("TUNED_RS_ROOT");
    }
}
