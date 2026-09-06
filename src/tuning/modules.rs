use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use tracing::{debug, info, warn};

use crate::config;
use crate::profile::PluginOptions;
use crate::rollback::Rollback;

const MODULES_FILE: &str = "/etc/modprobe.d/tuned.conf";

pub fn apply_options(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
    if options.is_empty() {
        return Ok(());
    }

    let path = config::resolve_path(MODULES_FILE);
    rollback.record_managed_file(&path)?;

    let leading_comments = read_leading_comments(&path)?;
    let mut output = leading_comments;
    let mut reload = Vec::new();
    let mut modinfo_available = true;

    for (module, raw) in options {
        validate_module_name(module)?;
        let (reload_module, parameters) = parse_module_value(raw);
        if modinfo_available {
            match Command::new("modinfo").arg(module).status() {
                Ok(status) if !status.success() => {
                    warn!("Kernel module '{module}' was not found; skipping it");
                    continue;
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    modinfo_available = false;
                    warn!("modinfo is unavailable; module existence checks are disabled");
                }
                Err(error) => return Err(error).context("Failed to execute modinfo"),
            }
        }

        if !parameters.is_empty() {
            output.push_str("options ");
            output.push_str(module);
            output.push(' ');
            output.push_str(&parameters);
            output.push('\n');
        }
        if reload_module {
            reload.push(module.clone());
        }
    }

    write_atomic(&path, output.as_bytes())?;
    for module in reload {
        reload_module(&module)?;
    }
    Ok(())
}

pub fn verify_options(options: &PluginOptions, ignore_missing: bool) -> bool {
    let mut verified = true;
    for (module, raw) in options {
        if validate_module_name(module).is_err() {
            verified = false;
            continue;
        }
        let module_path = config::resolve_path(&format!("/sys/module/{module}"));
        if !module_path.is_dir() {
            if !ignore_missing {
                warn!("Kernel module '{module}' is not loaded");
                verified = false;
            }
            continue;
        }
        let (_, parameters) = parse_module_value(raw);
        for assignment in parameters.split_whitespace() {
            let Some((name, expected)) = assignment.split_once('=') else {
                continue;
            };
            if !valid_parameter_name(name) {
                verified = false;
                continue;
            }
            let path = module_path.join("parameters").join(name.replace('/', ""));
            match fs::read_to_string(&path) {
                Ok(actual) if actual.trim() == expected => {}
                Ok(actual) => {
                    warn!(
                        "Module parameter mismatch: {} expected '{}' actual '{}'",
                        path.display(),
                        expected,
                        actual.trim()
                    );
                    verified = false;
                }
                Err(error) if ignore_missing && error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    debug!("Module parameter {} is not exported", path.display());
                }
                Err(error) => {
                    warn!("Cannot read module parameter {}: {error}", path.display());
                    verified = false;
                }
            }
        }
    }
    verified
}

fn parse_module_value(raw: &str) -> (bool, String) {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix("+r") else {
        return (false, trimmed.to_string());
    };
    let parameters = rest.trim_start().strip_prefix(',').unwrap_or(rest).trim();
    (true, parameters.to_string())
}

fn validate_module_name(module: &str) -> Result<()> {
    if module.is_empty()
        || !module
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        bail!("Invalid kernel module name '{module}'");
    }
    Ok(())
}

fn valid_parameter_name(parameter: &str) -> bool {
    !parameter.is_empty()
        && parameter
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn read_leading_comments(path: &Path) -> Result<String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", path.display()))
        }
    };
    let mut comments = String::new();
    for line in content.lines() {
        if line.trim_start().starts_with('#') || line.trim().is_empty() {
            comments.push_str(line);
            comments.push('\n');
        } else {
            break;
        }
    }
    Ok(comments)
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    fs::write(&temporary, contents)
        .with_context(|| format!("Failed to write {}", temporary.display()))?;
    fs::rename(&temporary, path)
        .with_context(|| format!("Failed to replace {}", path.display()))?;
    info!("Updated module policy in {}", path.display());
    Ok(())
}

fn reload_module(module: &str) -> Result<()> {
    match Command::new("modprobe").args(["-r", module]).status() {
        Ok(status) if !status.success() => {
            debug!("Kernel module '{module}' could not be removed before reload");
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            warn!("modprobe is unavailable; reboot is required to apply '{module}' options");
            return Ok(());
        }
        Err(error) => return Err(error).context("Failed to execute modprobe -r"),
    }

    let status = Command::new("modprobe")
        .arg(module)
        .status()
        .with_context(|| format!("Failed to reload kernel module '{module}'"))?;
    if !status.success() {
        warn!("Kernel module '{module}' could not be reloaded; reboot is required");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reload_prefix_and_parameters() {
        assert_eq!(
            parse_module_value("+r, nr_ndevs=2"),
            (true, "nr_ndevs=2".to_string())
        );
        assert_eq!(parse_module_value("+r"), (true, String::new()));
        assert_eq!(
            parse_module_value("power_save=1"),
            (false, "power_save=1".to_string())
        );
    }

    #[test]
    fn rejects_module_and_parameter_path_injection() {
        assert!(validate_module_name("snd_hda_intel").is_ok());
        assert!(validate_module_name("../evil").is_err());
        assert!(valid_parameter_name("power_save"));
        assert!(!valid_parameter_name("../../secret"));
    }
}
