use std::path::PathBuf;

use anyhow::Result;

use crate::config;
use crate::profile::PluginOptions;
use crate::profile_units::option_value;
use crate::rollback::Rollback;
use crate::tuning::generic_sysfs;

pub fn apply_options(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
    let Some(raw) = option_value(options, "avc_cache_threshold") else {
        return Ok(());
    };
    let value = raw.parse::<u64>().map_err(|_| {
        anyhow::anyhow!("SELinux AVC cache threshold must be a non-negative integer")
    })?;
    let Some(path) = cache_threshold_path() else {
        return Ok(());
    };
    generic_sysfs::apply_options(rollback, &vec![(logical_path(&path), value.to_string())])
}

pub fn verify_options(options: &PluginOptions, ignore_missing: bool) -> bool {
    let Some(raw) = option_value(options, "avc_cache_threshold") else {
        return true;
    };
    if raw.parse::<u64>().is_err() {
        return false;
    }
    let Some(path) = cache_threshold_path() else {
        return ignore_missing;
    };
    generic_sysfs::read_active_value(&path).is_ok_and(|actual| actual == raw)
}

fn cache_threshold_path() -> Option<PathBuf> {
    [
        "/sys/fs/selinux/avc/cache_threshold",
        "/selinux/avc/cache_threshold",
    ]
    .into_iter()
    .map(config::resolve_path)
    .find(|path| path.is_file())
}

fn logical_path(path: &std::path::Path) -> String {
    if std::env::var_os("TUNED_RS_ROOT").is_some() {
        let root = config::resolve_path("/");
        format!("/{}", path.strip_prefix(root).unwrap_or(path).display())
    } else {
        path.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_negative_and_non_numeric_thresholds() {
        for value in ["-1", "many"] {
            let options = vec![("avc_cache_threshold".to_string(), value.to_string())];
            assert!(!verify_options(&options, true));
        }
    }
}
