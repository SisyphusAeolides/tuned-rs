use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::{parse_assignment, read_trimmed, resolve_numeric_assignment};
use crate::tuning::sysfs::write_raw as write_sysfs_raw;

const VM_OPTIONS: &[&str] = &[
    "dirty_ratio",
    "dirty_background_ratio",
    "dirty_bytes",
    "dirty_background_bytes",
];

const THP_VALUES: &[&str] = &["always", "never", "madvise"];

pub fn write_raw(option: &str, value: &str) -> Result<()> {
    if option == "transparent_hugepages" {
        return write_transparent_hugepages(value);
    }
    let path = vm_path(option)?;
    if read_trimmed(&path).is_ok_and(|current| current == value.trim()) {
        info!("Keeping vm '{option}' at '{}'", value.trim());
        return Ok(());
    }
    info!("Writing '{value}' to {}", path.display());
    fs::write(&path, value).with_context(|| format!("Failed to write to {}", path.display()))?;
    Ok(())
}

pub fn apply_options(rollback: &Rollback, options: &[(String, String)]) -> Result<()> {
    for (first, second) in [
        ("dirty_bytes", "dirty_ratio"),
        ("dirty_background_bytes", "dirty_background_ratio"),
    ] {
        if options.iter().any(|(option, _)| option == first)
            && options.iter().any(|(option, _)| option == second)
        {
            warn!("Conflicting vm options '{first}' and '{second}' may cause undefined behavior");
        }
    }
    for (option, value) in options {
        apply_option(rollback, option, value)?;
    }
    Ok(())
}

pub fn apply_option(rollback: &Rollback, option: &str, raw_value: &str) -> Result<()> {
    match option {
        "transparent_hugepages" | "transparent_hugepage" => {
            apply_transparent_hugepages(rollback, raw_value)
        }
        "transparent_hugepage.defrag" => apply_transparent_hugepage_defrag(rollback, raw_value),
        other if VM_OPTIONS.contains(&other) => {
            let (effective_option, effective_value) = effective_option_value(other, raw_value);
            apply_vm_sysctl(rollback, effective_option, &effective_value)
        }
        other => {
            warn!("Unsupported vm option '{other}'");
            Ok(())
        }
    }
}

fn apply_vm_sysctl(rollback: &Rollback, option: &str, raw_value: &str) -> Result<()> {
    let path = match vm_path(option) {
        Ok(path) => path,
        Err(error) => {
            warn!("Skipping vm option '{option}': {error}");
            return Ok(());
        }
    };

    let assignment = parse_assignment(raw_value);
    let current = read_trimmed(&path)?;
    let Some(resolved) = resolve_numeric_assignment(&assignment, &current)? else {
        info!("Keeping vm '{option}' at '{current}'");
        return Ok(());
    };

    if current == resolved {
        info!("Keeping vm '{option}' at '{current}'");
        return Ok(());
    }

    if let Err(error) = validate_dirty_value(option, &resolved) {
        warn!("Skipping vm option '{option}': {error}");
        return Ok(());
    }

    if current == "0" {
        let counterpart = dirty_counterpart(option).expect("all vm options have counterparts");
        let counterpart_value = read_trimmed(&vm_path(counterpart)?)?;
        rollback.record_original(&rollback_key("vm", counterpart), &counterpart_value)?;
    } else {
        rollback.record_original(&rollback_key("vm", option), &current)?;
    }
    write_raw(option, &resolved)
}

pub(crate) fn effective_option_value<'a>(option: &'a str, raw_value: &str) -> (&'a str, String) {
    let value = raw_value.trim();
    match option {
        "dirty_bytes" if value.ends_with('%') => {
            ("dirty_ratio", value.trim_end_matches('%').to_string())
        }
        "dirty_background_bytes" if value.ends_with('%') => (
            "dirty_background_ratio",
            value.trim_end_matches('%').to_string(),
        ),
        _ => (option, value.to_string()),
    }
}

fn dirty_counterpart(option: &str) -> Option<&'static str> {
    match option {
        "dirty_bytes" => Some("dirty_ratio"),
        "dirty_ratio" => Some("dirty_bytes"),
        "dirty_background_bytes" => Some("dirty_background_ratio"),
        "dirty_background_ratio" => Some("dirty_background_bytes"),
        _ => None,
    }
}

fn validate_dirty_value(option: &str, raw_value: &str) -> Result<()> {
    let value = raw_value
        .parse::<i64>()
        .with_context(|| format!("Value '{raw_value}' must be an integer"))?;
    match option {
        "dirty_ratio" | "dirty_background_ratio" if !(0..=100).contains(&value) => {
            bail!("Value must be between 0 and 100")
        }
        "dirty_bytes" if value < twice_page_size() => {
            bail!("Value must be at least twice the page size")
        }
        "dirty_background_bytes" if value <= 0 => bail!("Value must be positive"),
        _ => Ok(()),
    }
}

fn twice_page_size() -> i64 {
    // SAFETY: sysconf only reads the process's page-size configuration.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size > 0 {
        page_size.saturating_mul(2)
    } else {
        8192
    }
}

fn apply_transparent_hugepages(rollback: &Rollback, raw_value: &str) -> Result<()> {
    let value = raw_value.trim();
    if !THP_VALUES.contains(&value) {
        warn!("Unsupported transparent_hugepages value '{value}'");
        return Ok(());
    }
    if fs::read_to_string(crate::config::resolve_path("/proc/cmdline"))
        .unwrap_or_default()
        .contains("transparent_hugepage=")
    {
        info!("transparent_hugepage set in kernel cmdline; skipping profile value");
        return Ok(());
    }
    let path = thp_path()?.join("enabled");
    if !path.is_file() {
        warn!("transparent_hugepages is not supported on this system");
        return Ok(());
    }
    let current = read_trimmed(&path)?;
    rollback.record_original(
        &rollback_key("vm", "transparent_hugepages"),
        active_choice(&current),
    )?;
    write_sysfs_raw(&path, value)
}

fn apply_transparent_hugepage_defrag(rollback: &Rollback, raw_value: &str) -> Result<()> {
    let path = thp_path()?.join("defrag");
    if !path.is_file() {
        warn!("transparent_hugepage.defrag is not supported on this system");
        return Ok(());
    }
    let current = read_trimmed(&path)?;
    rollback.record_original(
        &rollback_key("vm", "transparent_hugepage.defrag"),
        active_choice(&current),
    )?;
    write_sysfs_raw(&path, raw_value.trim())
}

fn write_transparent_hugepages(value: &str) -> Result<()> {
    let path = thp_path()?.join("enabled");
    write_sysfs_raw(&path, value)
}

fn vm_path(option: &str) -> Result<PathBuf> {
    if !VM_OPTIONS.contains(&option) {
        bail!("Unsupported vm option '{option}'");
    }
    Ok(crate::config::resolve_path("/proc/sys/vm").join(option))
}

fn thp_path() -> Result<PathBuf> {
    for path in [
        "/sys/kernel/mm/transparent_hugepage",
        "/sys/kernel/mm/redhat_transparent_hugepage",
    ] {
        let path = crate::config::resolve_path(path);
        if path.is_dir() {
            return Ok(path);
        }
    }
    bail!("Transparent hugepage interface not found")
}

fn active_choice(raw: &str) -> &str {
    let Some(start) = raw.find('[') else {
        return raw.trim();
    };
    let Some(end) = raw[start + 1..].find(']') else {
        return raw.trim();
    };
    raw[start + 1..start + 1 + end].trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn create_vm_pair(root: &TempDir, option: &str, value: &str, counterpart: &str, other: &str) {
        let vm = root.path().join("proc/sys/vm");
        fs::create_dir_all(&vm).unwrap();
        fs::write(vm.join(option), value).unwrap();
        fs::write(vm.join(counterpart), other).unwrap();
    }

    #[test]
    fn percentage_bytes_use_the_ratio_interface() {
        assert_eq!(
            effective_option_value("dirty_bytes", "40%"),
            ("dirty_ratio", "40".to_string())
        );
        assert_eq!(
            effective_option_value("dirty_background_bytes", "10%"),
            ("dirty_background_ratio", "10".to_string())
        );
    }

    #[test]
    fn rollback_preserves_the_active_counterpart() {
        let _env_guard = crate::config::test_env_lock();
        let root = TempDir::new().unwrap();
        create_vm_pair(&root, "dirty_bytes", "0", "dirty_ratio", "20");
        std::env::set_var("TUNED_RS_ROOT", root.path());

        let rollback = Rollback::load().unwrap();
        apply_option(&rollback, "dirty_bytes", "8192").unwrap();
        fs::write(root.path().join("proc/sys/vm/dirty_ratio"), "0").unwrap();
        rollback.restore_all().unwrap();

        assert_eq!(
            fs::read_to_string(root.path().join("proc/sys/vm/dirty_ratio")).unwrap(),
            "20"
        );
        std::env::remove_var("TUNED_RS_ROOT");
    }

    #[test]
    fn stale_zero_rollback_is_an_idempotent_success() {
        let _env_guard = crate::config::test_env_lock();
        let root = TempDir::new().unwrap();
        create_vm_pair(&root, "dirty_bytes", "0", "dirty_ratio", "20");
        std::env::set_var("TUNED_RS_ROOT", root.path());
        let path = root.path().join("proc/sys/vm/dirty_bytes");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();

        write_raw("dirty_bytes", "0").unwrap();

        std::env::remove_var("TUNED_RS_ROOT");
    }

    #[test]
    fn transparent_hugepage_apply_and_rollback_use_the_active_choice() {
        let _env_guard = crate::config::test_env_lock();
        let root = TempDir::new().unwrap();
        let thp = root.path().join("sys/kernel/mm/transparent_hugepage");
        fs::create_dir_all(&thp).unwrap();
        fs::create_dir_all(root.path().join("proc")).unwrap();
        fs::write(root.path().join("proc/cmdline"), "quiet").unwrap();
        fs::write(thp.join("enabled"), "[always] madvise never\n").unwrap();
        std::env::set_var("TUNED_RS_ROOT", root.path());

        let rollback = Rollback::load().unwrap();
        apply_option(&rollback, "transparent_hugepage", "never").unwrap();
        assert_eq!(fs::read_to_string(thp.join("enabled")).unwrap(), "never");
        rollback.restore_all().unwrap();
        assert_eq!(fs::read_to_string(thp.join("enabled")).unwrap(), "always");

        std::env::remove_var("TUNED_RS_ROOT");
    }
}
