use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config;
use crate::profile::PluginOptions;
use crate::profile_units::option_value;
use crate::rollback::Rollback;

const DROP_IN: &str = "/etc/systemd/system.conf.d/00-tuned.conf";
const HEADER: &str = "# This file is managed by tuned-rs.\n[Manager]\n";

pub fn apply_options(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
    let Some(raw) = option_value(options, "cpu_affinity") else {
        return Ok(());
    };
    let affinity = normalize_cpu_list(raw)?;
    let path = config::resolve_path(DROP_IN);
    let current = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => HEADER.to_string(),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", path.display()))
        }
    };
    let updated = replace_manager_value(&current, "CPUAffinity", &affinity);
    if updated == current {
        return Ok(());
    }
    rollback.record_managed_file(&path)?;
    atomic_write(&path, &updated)
}

pub fn verify_options(options: &PluginOptions, ignore_missing: bool) -> bool {
    let Some(raw) = option_value(options, "cpu_affinity") else {
        return true;
    };
    let Ok(expected) = normalize_cpu_list(raw) else {
        return false;
    };
    let path = config::resolve_path(DROP_IN);
    let Ok(contents) = fs::read_to_string(path) else {
        return ignore_missing;
    };
    manager_value(&contents, "CPUAffinity")
        .and_then(|value| normalize_cpu_list(value).ok())
        .is_some_and(|actual| actual == expected)
}

fn normalize_cpu_list(raw: &str) -> Result<String> {
    let mut cpus = Vec::new();
    for item in raw.split([',', ' ', '\t']).filter(|item| !item.is_empty()) {
        if let Some((start, end)) = item.split_once('-') {
            let start = parse_cpu(start)?;
            let end = parse_cpu(end)?;
            if start > end || end - start > 1_048_576 {
                bail!("Invalid CPU range '{item}'");
            }
            cpus.extend(start..=end);
        } else {
            cpus.push(parse_cpu(item)?);
        }
    }
    if cpus.is_empty() {
        bail!("CPUAffinity must select at least one CPU");
    }
    cpus.sort_unstable();
    cpus.dedup();
    Ok(cpus
        .into_iter()
        .map(|cpu| cpu.to_string())
        .collect::<Vec<_>>()
        .join(" "))
}

fn parse_cpu(raw: &str) -> Result<u32> {
    raw.parse::<u32>()
        .with_context(|| format!("Invalid CPU identifier '{raw}'"))
}

fn manager_value<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    let mut in_manager = false;
    let mut found = None;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_manager = trimmed.eq_ignore_ascii_case("[Manager]");
            continue;
        }
        if !in_manager || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let Some((name, value)) = trimmed.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case(key) {
            found = Some(value.trim());
        }
    }
    found
}

fn replace_manager_value(contents: &str, key: &str, value: &str) -> String {
    let mut output = Vec::new();
    let mut in_manager = false;
    let mut manager_seen = false;
    let mut replaced = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_manager && !replaced {
                output.push(format!("{key}={value}"));
                replaced = true;
            }
            in_manager = trimmed.eq_ignore_ascii_case("[Manager]");
            manager_seen |= in_manager;
            output.push(line.to_string());
            continue;
        }
        if in_manager
            && trimmed
                .split_once('=')
                .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case(key))
        {
            if !replaced {
                output.push(format!("{key}={value}"));
                replaced = true;
            }
        } else {
            output.push(line.to_string());
        }
    }
    if !manager_seen {
        if !output.is_empty() && output.last().is_some_and(|line| !line.is_empty()) {
            output.push(String::new());
        }
        output.push("[Manager]".to_string());
    }
    if !replaced {
        output.push(format!("{key}={value}"));
    }
    format!("{}\n", output.join("\n"))
}

fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid systemd drop-in path"))?;
    fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    fs::write(&temporary, contents)
        .with_context(|| format!("Failed to write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("Failed to replace {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_and_normalizes_cpu_ranges() {
        assert_eq!(normalize_cpu_list("3,0-2,2").unwrap(), "0 1 2 3");
        assert!(normalize_cpu_list("4-2").is_err());
        assert!(normalize_cpu_list("0,whoops").is_err());
    }

    #[test]
    fn replaces_only_the_manager_cpu_affinity() {
        let input = "[Other]\nCPUAffinity=7\n[Manager]\nFoo=bar\nCPUAffinity=4-5\n";
        let output = replace_manager_value(input, "CPUAffinity", "0 1");
        assert!(output.contains("[Other]\nCPUAffinity=7"));
        assert_eq!(manager_value(&output, "CPUAffinity"), Some("0 1"));
        assert_eq!(output.matches("CPUAffinity=0 1").count(), 1);
    }
}
