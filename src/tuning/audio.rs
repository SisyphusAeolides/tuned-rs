use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use tracing::{debug, info, warn};

use crate::config;
use crate::profile::PluginOptions;
use crate::profile_units::option_value;
use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::read_trimmed;

const AUDIO_MODULES: &[&str] = &["snd_hda_intel", "snd_ac97_codec"];

pub fn apply_options(rollback: &Rollback, devices: &str, options: &PluginOptions) -> Result<()> {
    let timeout = option_value(options, "timeout")
        .unwrap_or("0")
        .parse::<i64>()
        .context("Audio timeout must be an integer")?;
    if timeout < 0 {
        bail!("Audio timeout must not be negative");
    }
    let reset_controller = option_value(options, "reset_controller")
        .map(parse_bool)
        .transpose()?
        .unwrap_or(true);

    let mut updated = 0usize;
    let modules = crate::device_matcher::filter_names(
        devices,
        AUDIO_MODULES.iter().map(|module| (*module).to_string()),
    );
    for module in modules {
        let base = config::resolve_path(&format!("/sys/module/{module}/parameters"));
        let timeout_path = base.join("power_save");
        if timeout_path.is_file() {
            write_node(rollback, &timeout_path, &timeout.to_string())?;
            updated += 1;
        }
        let reset_path = base.join("power_save_controller");
        if reset_path.is_file() {
            write_node(
                rollback,
                &reset_path,
                if reset_controller { "1" } else { "0" },
            )?;
            updated += 1;
        }
    }

    if updated == 0 {
        debug!("No supported audio power-management module is loaded");
    } else {
        info!("Updated {updated} audio power-management control(s)");
    }
    Ok(())
}

pub fn verify_options(options: &PluginOptions, ignore_missing: bool) -> bool {
    let timeout = match option_value(options, "timeout")
        .unwrap_or("0")
        .parse::<i64>()
    {
        Ok(value) if value >= 0 => value.to_string(),
        _ => return false,
    };
    let reset = match option_value(options, "reset_controller") {
        Some(value) => match parse_bool(value) {
            Ok(value) => Some(if value { "1" } else { "0" }),
            Err(_) => return false,
        },
        None => Some("1"),
    };

    let mut found = false;
    let mut verified = true;
    for module in AUDIO_MODULES {
        let base = config::resolve_path(&format!("/sys/module/{module}/parameters"));
        for (leaf, expected) in [
            ("power_save", Some(timeout.as_str())),
            ("power_save_controller", reset),
        ] {
            let path = base.join(leaf);
            if !path.is_file() {
                continue;
            }
            found = true;
            match read_trimmed(&path) {
                Ok(actual) if Some(actual.as_str()) == expected => {}
                Ok(actual) => {
                    warn!(
                        "Audio control mismatch at {}: expected {:?}, actual '{}'",
                        path.display(),
                        expected,
                        actual
                    );
                    verified = false;
                }
                Err(error) => {
                    warn!("Cannot read audio control {}: {error}", path.display());
                    verified = false;
                }
            }
        }
    }
    verified && (found || ignore_missing)
}

fn write_node(rollback: &Rollback, path: &Path, value: &str) -> Result<()> {
    let original = read_trimmed(path)?;
    if original == value {
        return Ok(());
    }
    rollback.record_original(&rollback_key("sysfs", &path.to_string_lossy()), &original)?;
    fs::write(path, value).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

fn parse_bool(raw: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "y" | "yes" | "t" | "true" | "on" => Ok(true),
        "0" | "n" | "no" | "f" | "false" | "off" => Ok(false),
        _ => bail!("Invalid boolean value '{raw}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parses_upstream_boolean_spellings() {
        assert!(parse_bool("true").unwrap());
        assert!(!parse_bool("0").unwrap());
        assert!(parse_bool("sometimes").is_err());
    }

    #[test]
    fn device_selector_limits_module_controls_and_rolls_back() {
        let _env_guard = crate::config::test_env_lock();
        let root = TempDir::new().unwrap();
        for module in AUDIO_MODULES {
            let parameters = root
                .path()
                .join("sys/module")
                .join(module)
                .join("parameters");
            fs::create_dir_all(&parameters).unwrap();
            fs::write(parameters.join("power_save"), "0").unwrap();
            fs::write(parameters.join("power_save_controller"), "0").unwrap();
        }
        std::env::set_var("TUNED_RS_ROOT", root.path());
        let rollback = Rollback::load().unwrap();
        let options = vec![
            ("timeout".to_string(), "10".to_string()),
            ("reset_controller".to_string(), "true".to_string()),
        ];
        apply_options(&rollback, "snd_hda_intel", &options).unwrap();
        assert_eq!(
            fs::read_to_string(
                root.path()
                    .join("sys/module/snd_hda_intel/parameters/power_save")
            )
            .unwrap(),
            "10"
        );
        assert_eq!(
            fs::read_to_string(
                root.path()
                    .join("sys/module/snd_ac97_codec/parameters/power_save")
            )
            .unwrap(),
            "0"
        );
        rollback.restore_all().unwrap();
        assert_eq!(
            fs::read_to_string(
                root.path()
                    .join("sys/module/snd_hda_intel/parameters/power_save")
            )
            .unwrap(),
            "0"
        );
        std::env::remove_var("TUNED_RS_ROOT");
    }
}
