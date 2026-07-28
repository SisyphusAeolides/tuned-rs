use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::config;
use crate::profile::PluginOptions;
use crate::rollback::{rollback_key, Rollback};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Directive {
    running: Option<bool>,
    enabled: Option<bool>,
    file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServiceState {
    running: bool,
    enabled: bool,
}

pub fn apply_options(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
    for (option, raw) in options {
        let Some(name) = option.strip_prefix("service.") else {
            bail!("Invalid service option '{option}'");
        };
        validate_service_name(name)?;
        let directive = parse_directive(raw)?;
        let state = query_state(name)?;
        rollback.record_original(
            &rollback_key("service", name),
            &serde_json::to_string(&state)?,
        )?;
        if let Some(source) = directive.file.as_deref() {
            install_dropin(rollback, name, source)?;
        }
        apply_state(name, directive.running, directive.enabled)?;
    }
    Ok(())
}

pub fn verify_options(options: &PluginOptions, ignore_missing: bool) -> bool {
    options.iter().all(|(option, raw)| {
        let Some(name) = option.strip_prefix("service.") else {
            return false;
        };
        let Ok(directive) = parse_directive(raw) else {
            return false;
        };
        let Ok(state) = query_state(name) else {
            return ignore_missing;
        };
        directive
            .running
            .map_or(true, |expected| state.running == expected)
            && directive
                .enabled
                .map_or(true, |expected| state.enabled == expected)
            && directive
                .file
                .as_deref()
                .map_or(true, |source| dropin_matches(name, source))
    })
}

pub fn restore_state(name: &str, encoded: &str) -> Result<()> {
    validate_service_name(name)?;
    let state: ServiceState = serde_json::from_str(encoded)?;
    apply_state(name, Some(state.running), Some(state.enabled))
}

pub fn daemon_reload() -> Result<()> {
    if std::env::var_os("TUNED_RS_ROOT").is_some() {
        return Ok(());
    }
    command(&["daemon-reload"])
}

fn parse_directive(raw: &str) -> Result<Directive> {
    let mut directive = Directive::default();
    for item in raw
        .split([',', ';'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        match item {
            "start" => directive.running = Some(true),
            "stop" => directive.running = Some(false),
            "enable" => directive.enabled = Some(true),
            "disable" => directive.enabled = Some(false),
            _ if item.starts_with("file:") => {
                let path = item[5..].trim();
                if path.is_empty() {
                    bail!("Empty service configuration overlay path");
                }
                directive.file = Some(PathBuf::from(path));
            }
            _ => bail!("Invalid service directive '{item}'"),
        }
    }
    Ok(directive)
}

fn validate_service_name(name: &str) -> Result<()> {
    if !name.is_empty()
        && name.len() <= 255
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'@' | b':' | b'\\')
        })
        && !name.contains("..")
        && !name.contains(['/', '\0'])
    {
        Ok(())
    } else {
        bail!("Invalid service name '{name}'")
    }
}

fn query_state(name: &str) -> Result<ServiceState> {
    if std::env::var_os("TUNED_RS_ROOT").is_some() {
        return Ok(ServiceState {
            running: false,
            enabled: false,
        });
    }
    Ok(ServiceState {
        running: command_status(&["is-active", "--quiet", name])?,
        enabled: command_status(&["is-enabled", "--quiet", name])?,
    })
}

fn apply_state(name: &str, running: Option<bool>, enabled: Option<bool>) -> Result<()> {
    if std::env::var_os("TUNED_RS_ROOT").is_some() {
        return Ok(());
    }
    if let Some(running) = running {
        command(&[if running { "restart" } else { "stop" }, name])?;
    }
    if let Some(enabled) = enabled {
        command(&[if enabled { "enable" } else { "disable" }, name])?;
    }
    Ok(())
}

fn command(arguments: &[&str]) -> Result<()> {
    let status = Command::new("systemctl").args(arguments).status()?;
    if status.success() {
        Ok(())
    } else {
        bail!("systemctl {} failed with {status}", arguments.join(" "))
    }
}

fn command_status(arguments: &[&str]) -> Result<bool> {
    let status = Command::new("systemctl").args(arguments).status()?;
    match status.code() {
        Some(0) => Ok(true),
        Some(_) => Ok(false),
        None => bail!("systemctl was terminated by a signal"),
    }
}

fn install_dropin(rollback: &Rollback, service: &str, source: &Path) -> Result<()> {
    let source = validate_source(source)?;
    let file_name = source
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Invalid overlay filename"))?;
    let destination = config::resolve_path("/etc/systemd/system")
        .join(format!("{service}.service.d"))
        .join(file_name);
    rollback.record_systemd_dropin(&destination)?;
    fs::create_dir_all(destination.parent().unwrap())?;
    let temporary = destination.with_extension("tuned-rs-new");
    fs::copy(source, &temporary)?;
    fs::rename(temporary, destination)?;
    daemon_reload()
}

fn validate_source(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("Service overlay path must be absolute");
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Missing service overlay {}", path.display()))?;
    if !canonical.is_file() {
        bail!("Service overlay is not a regular file");
    }
    let allowed = config::profile_dirs_from_env()
        .into_iter()
        .map(config::resolve_path_buf)
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| canonical.starts_with(root));
    if !allowed {
        bail!("Service overlay is outside configured profile directories");
    }
    Ok(canonical)
}

fn dropin_matches(service: &str, source: &Path) -> bool {
    let Ok(source) = validate_source(source) else {
        return false;
    };
    let Some(file_name) = source.file_name() else {
        return false;
    };
    let destination = config::resolve_path("/etc/systemd/system")
        .join(format!("{service}.service.d"))
        .join(file_name);
    fs::read(source)
        .ok()
        .zip(fs::read(destination).ok())
        .is_some_and(|(a, b)| a == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_last_service_directive_and_overlay() {
        let value = parse_directive("start,stop;enable,file:/profiles/x.conf").unwrap();
        assert_eq!(value.running, Some(false));
        assert_eq!(value.enabled, Some(true));
        assert_eq!(value.file, Some(PathBuf::from("/profiles/x.conf")));
    }

    #[test]
    fn rejects_service_name_and_directive_injection() {
        assert!(validate_service_name("../../evil").is_err());
        assert!(validate_service_name("sshd").is_ok());
        assert!(parse_directive("start,$(evil)").is_err());
    }
}
