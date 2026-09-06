use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config;
use crate::profile::PluginOptions;
use crate::profile_units::option_value;
use crate::rollback::Rollback;

const SYSCONFIG: &str = "/etc/sysconfig/irqbalance";
const VARIABLE: &str = "IRQBALANCE_BANNED_CPULIST";

pub fn apply_options(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
    let Some(raw) = option_value(options, "banned_cpus") else {
        return Ok(());
    };
    let value = canonical_cpu_list(raw)?;
    validate_present_cpus(&value)?;
    let path = config::resolve_path(SYSCONFIG);
    let current = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", path.display()))
        }
    };
    let updated = replace_assignment(&current, VARIABLE, &value);
    if updated == current {
        return Ok(());
    }
    rollback.record_managed_file(&path)?;
    atomic_write(&path, &updated)?;
    try_restart()
}

pub fn verify_options(options: &PluginOptions, ignore_missing: bool) -> bool {
    let Some(raw) = option_value(options, "banned_cpus") else {
        return true;
    };
    let Ok(expected) = canonical_cpu_list(raw) else {
        return false;
    };
    let path = config::resolve_path(SYSCONFIG);
    let Ok(contents) = fs::read_to_string(path) else {
        return ignore_missing;
    };
    assignment(&contents, VARIABLE)
        .and_then(|actual| canonical_cpu_list(actual).ok())
        .is_some_and(|actual| actual == expected)
}

pub(crate) fn try_restart() -> Result<()> {
    if std::env::var_os("TUNED_RS_ROOT").is_some() {
        return Ok(());
    }
    let status = Command::new("systemctl")
        .args(["try-restart", "irqbalance.service"])
        .status()
        .context("Failed to execute systemctl for irqbalance")?;
    if status.success() || status.code() == Some(5) {
        Ok(())
    } else {
        bail!("systemctl try-restart irqbalance failed with {status}")
    }
}

fn validate_present_cpus(canonical: &str) -> Result<()> {
    let path = config::resolve_path("/sys/devices/system/cpu/possible");
    let Ok(possible) = fs::read_to_string(path) else {
        return Ok(());
    };
    let requested = expand_cpu_list(canonical)?;
    let possible = expand_cpu_list(possible.trim())?;
    if requested
        .iter()
        .all(|cpu| possible.binary_search(cpu).is_ok())
    {
        Ok(())
    } else {
        bail!("banned_cpus selects a CPU outside the possible CPU set")
    }
}

fn canonical_cpu_list(raw: &str) -> Result<String> {
    let cpus = expand_cpu_list(raw)?;
    if cpus.is_empty() {
        bail!("banned_cpus must select at least one CPU");
    }
    let mut ranges = Vec::new();
    let mut start = cpus[0];
    let mut previous = cpus[0];
    for &cpu in &cpus[1..] {
        if cpu == previous + 1 {
            previous = cpu;
            continue;
        }
        ranges.push(format_range(start, previous));
        start = cpu;
        previous = cpu;
    }
    ranges.push(format_range(start, previous));
    Ok(ranges.join(","))
}

fn expand_cpu_list(raw: &str) -> Result<Vec<u32>> {
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
    cpus.sort_unstable();
    cpus.dedup();
    Ok(cpus)
}

fn parse_cpu(raw: &str) -> Result<u32> {
    raw.parse::<u32>()
        .with_context(|| format!("Invalid CPU identifier '{raw}'"))
}

fn format_range(start: u32, end: u32) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

fn assignment<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    contents.lines().rev().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            return None;
        }
        let (name, value) = trimmed.split_once('=')?;
        (name.trim() == key).then_some(value.trim().trim_matches(['\'', '"']))
    })
}

fn replace_assignment(contents: &str, key: &str, value: &str) -> String {
    let mut lines = contents
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed.starts_with('#')
                || trimmed
                    .split_once('=')
                    .map_or(true, |(name, _)| name.trim() != key)
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    lines.push(format!("{key}={value}"));
    format!("{}\n", lines.join("\n"))
}

fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    fs::write(&temporary, contents)
        .with_context(|| format!("Failed to write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("Failed to replace {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_cpu_lists_and_ranges() {
        assert_eq!(canonical_cpu_list("4,2,3,9-11,10").unwrap(), "2-4,9-11");
        assert!(canonical_cpu_list("7-2").is_err());
    }

    #[test]
    fn replaces_duplicate_active_assignments_but_keeps_comments() {
        let input = "# IRQBALANCE_BANNED_CPULIST=old\nX=1\nIRQBALANCE_BANNED_CPULIST=2\n";
        let output = replace_assignment(input, VARIABLE, "3-4");
        assert!(output.contains("# IRQBALANCE_BANNED_CPULIST=old"));
        assert_eq!(assignment(&output, VARIABLE), Some("3-4"));
        assert_eq!(output.matches("\nIRQBALANCE_BANNED_CPULIST=").count(), 1);
    }
}
