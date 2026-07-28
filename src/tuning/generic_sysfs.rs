use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use tracing::{debug, info, warn};

use crate::config;
use crate::profile::PluginOptions;
use crate::rollback::{rollback_key, Rollback};
use crate::tuning::modifiers::{
    parse_assignment, read_trimmed, resolve_numeric_assignment, AssignmentOp,
};
use crate::tuning::sysfs;

pub fn apply_options(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
    for (pattern, raw_value) in options {
        validate_payload(raw_value)?;
        let targets = expand_pattern(pattern)?;
        if targets.is_empty() {
            warn!("Generic sysfs pattern '{pattern}' matched no controls");
            continue;
        }

        for target in targets {
            let current = read_active_value(&target)?;
            let assignment = parse_assignment(raw_value);
            let Some(resolved) = resolve_numeric_assignment(&assignment, &current)? else {
                debug!(
                    "Keeping generic sysfs control {} at '{}'",
                    target.display(),
                    current
                );
                continue;
            };
            validate_payload(&resolved)?;
            if equivalent_value(&resolved, &current) {
                continue;
            }

            rollback.record_original(
                &rollback_key("sysfs", &target.to_string_lossy()),
                &current,
            )?;
            sysfs::write_raw(&target, &resolved)?;
            info!(
                "Set generic sysfs control {} to '{}'",
                target.display(),
                resolved
            );
        }
    }
    Ok(())
}

pub(crate) fn expand_pattern(pattern: &str) -> Result<Vec<PathBuf>> {
    expand_pattern_under(pattern, &config::resolve_path("/sys"))
}

fn expand_pattern_under(pattern: &str, sys_root: &Path) -> Result<Vec<PathBuf>> {
    validate_pattern(pattern)?;
    if !sys_root.is_dir() {
        return Ok(Vec::new());
    }

    let logical = Path::new(pattern);
    let relative = logical
        .strip_prefix("/sys")
        .with_context(|| format!("Generic sysfs path must be rooted below /sys: {pattern}"))?;
    if relative.as_os_str().is_empty() {
        bail!("Generic sysfs path must identify a control below /sys");
    }

    let mut candidates = vec![sys_root.to_path_buf()];
    for component in relative.components() {
        let Component::Normal(component) = component else {
            bail!("Invalid generic sysfs path component in '{pattern}'");
        };
        let component = component
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Generic sysfs paths must be valid UTF-8"))?;
        let wildcard = contains_meta(component);
        let mut next = Vec::new();

        for base in candidates {
            if wildcard {
                let entries = match fs::read_dir(&base) {
                    Ok(entries) => entries,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("Failed to expand {}", base.display()))
                    }
                };
                for entry in entries {
                    let entry = entry?;
                    let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                        continue;
                    };
                    if glob_component_matches(component, &name) {
                        next.push(entry.path());
                    }
                }
            } else {
                let candidate = base.join(component);
                if candidate.exists() {
                    next.push(candidate);
                }
            }
        }
        candidates = next;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
    }

    let canonical_root = sys_root
        .canonicalize()
        .with_context(|| format!("Failed to resolve sysfs root {}", sys_root.display()))?;
    let mut resolved = Vec::new();
    for candidate in candidates {
        let canonical = match candidate.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to resolve {}", candidate.display()))
            }
        };
        if canonical == canonical_root || !canonical.starts_with(&canonical_root) {
            bail!(
                "Refusing generic sysfs path outside {}: {}",
                canonical_root.display(),
                canonical.display()
            );
        }
        if canonical.is_file() {
            resolved.push(canonical);
        }
    }
    resolved.sort_unstable();
    resolved.dedup();
    Ok(resolved)
}

pub(crate) fn read_active_value(path: &Path) -> Result<String> {
    let raw = read_trimmed(path)?;
    Ok(active_value(&raw).to_string())
}

pub(crate) fn value_matches(expected: &str, actual: &str) -> bool {
    let assignment = parse_assignment(expected);
    match assignment.op {
        AssignmentOp::Set => assignment
            .target
            .split('|')
            .map(str::trim)
            .filter(|candidate| !candidate.is_empty())
            .any(|candidate| equivalent_value(candidate, actual)),
        AssignmentOp::Greater | AssignmentOp::GreaterEqual => {
            compare_numeric(&assignment.target, actual, |actual, target| {
                actual >= target
            })
        }
        AssignmentOp::Less | AssignmentOp::LessEqual => {
            compare_numeric(&assignment.target, actual, |actual, target| {
                actual <= target
            })
        }
    }
}

fn validate_pattern(pattern: &str) -> Result<()> {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern.len() > 4096 || pattern.contains('\0') {
        bail!("Invalid generic sysfs path pattern");
    }
    let path = Path::new(pattern);
    if !path.is_absolute() || !path.starts_with("/sys") {
        bail!("Generic sysfs paths must be absolute and rooted below /sys");
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::CurDir | Component::Prefix(_)
        )
    }) {
        bail!("Generic sysfs paths must not contain relative components");
    }
    Ok(())
}

fn validate_payload(payload: &str) -> Result<()> {
    if payload.is_empty() || payload.len() > 4096 {
        bail!("Invalid generic sysfs value");
    }
    if payload
        .chars()
        .any(|character| character == '\n' || character == '\0')
    {
        bail!("Generic sysfs values must not contain control characters");
    }
    Ok(())
}

fn contains_meta(component: &str) -> bool {
    component
        .bytes()
        .any(|byte| matches!(byte, b'*' | b'?' | b'['))
}

fn glob_component_matches(pattern: &str, text: &str) -> bool {
    fn matches_from(
        pattern: &[u8],
        text: &[u8],
        pattern_index: usize,
        text_index: usize,
        memo: &mut HashMap<(usize, usize), bool>,
    ) -> bool {
        if let Some(cached) = memo.get(&(pattern_index, text_index)) {
            return *cached;
        }

        let result = if pattern_index == pattern.len() {
            text_index == text.len()
        } else {
            match pattern[pattern_index] {
                b'*' => {
                    let mut next_pattern = pattern_index + 1;
                    while next_pattern < pattern.len() && pattern[next_pattern] == b'*' {
                        next_pattern += 1;
                    }
                    if next_pattern == pattern.len() {
                        true
                    } else {
                        (text_index..=text.len()).any(|next_text| {
                            matches_from(pattern, text, next_pattern, next_text, memo)
                        })
                    }
                }
                b'?' => {
                    text_index < text.len()
                        && matches_from(
                            pattern,
                            text,
                            pattern_index + 1,
                            text_index + 1,
                            memo,
                        )
                }
                b'[' if text_index < text.len() => match character_class(
                    pattern,
                    pattern_index,
                    text[text_index],
                ) {
                    Some((matched, next_pattern)) => {
                        matched
                            && matches_from(
                                pattern,
                                text,
                                next_pattern,
                                text_index + 1,
                                memo,
                            )
                    }
                    None => {
                        text[text_index] == b'['
                            && matches_from(
                                pattern,
                                text,
                                pattern_index + 1,
                                text_index + 1,
                                memo,
                            )
                    }
                },
                b'\\' if pattern_index + 1 < pattern.len() => {
                    text_index < text.len()
                        && text[text_index] == pattern[pattern_index + 1]
                        && matches_from(
                            pattern,
                            text,
                            pattern_index + 2,
                            text_index + 1,
                            memo,
                        )
                }
                byte => {
                    text_index < text.len()
                        && text[text_index] == byte
                        && matches_from(
                            pattern,
                            text,
                            pattern_index + 1,
                            text_index + 1,
                            memo,
                        )
                }
            }
        };
        memo.insert((pattern_index, text_index), result);
        result
    }

    matches_from(
        pattern.as_bytes(),
        text.as_bytes(),
        0,
        0,
        &mut HashMap::new(),
    )
}

fn character_class(pattern: &[u8], opening: usize, value: u8) -> Option<(bool, usize)> {
    let closing = pattern[opening + 1..]
        .iter()
        .position(|byte| *byte == b']')?
        + opening
        + 1;
    if closing == opening + 1 {
        return None;
    }

    let mut cursor = opening + 1;
    let negated = matches!(pattern[cursor], b'!' | b'^');
    if negated {
        cursor += 1;
    }
    let mut matched = false;
    while cursor < closing {
        if cursor + 2 < closing && pattern[cursor + 1] == b'-' {
            let start = pattern[cursor];
            let end = pattern[cursor + 2];
            if start <= value && value <= end {
                matched = true;
            }
            cursor += 3;
        } else {
            if pattern[cursor] == value {
                matched = true;
            }
            cursor += 1;
        }
    }
    Some((if negated { !matched } else { matched }, closing + 1))
}

fn active_value(raw: &str) -> &str {
    let Some(start) = raw.find('[') else {
        return raw.trim();
    };
    let Some(relative_end) = raw[start + 1..].find(']') else {
        return raw.trim();
    };
    raw[start + 1..start + 1 + relative_end].trim()
}

fn equivalent_value(expected: &str, actual: &str) -> bool {
    let expected = unquote(expected.trim());
    let actual = unquote(active_value(actual).trim());
    match (parse_number(expected), parse_number(actual)) {
        (Some(expected), Some(actual)) => expected == actual,
        _ => expected.split_whitespace().eq(actual.split_whitespace()),
    }
}

fn compare_numeric(target: &str, actual: &str, compare: impl Fn(i128, i128) -> bool) -> bool {
    match (parse_number(target), parse_number(active_value(actual))) {
        (Some(target), Some(actual)) => compare(actual, target),
        _ => equivalent_value(target, actual),
    }
}

fn parse_number(value: &str) -> Option<i128> {
    let value = unquote(value.trim());
    value.parse::<i128>().ok().or_else(|| {
        let hexadecimal = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
            .unwrap_or(value);
        i128::from_str_radix(hexadecimal, 16).ok()
    })
}

fn unquote(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
        {
            return &value[1..value.len() - 1];
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn matches_shell_style_path_components() {
        assert!(glob_component_matches("machinecheck*", "machinecheck0"));
        assert!(glob_component_matches("cpu[0-9]", "cpu7"));
        assert!(glob_component_matches("nvme?", "nvme0"));
        assert!(!glob_component_matches("cpu[!0-3]", "cpu2"));
        assert!(glob_component_matches("cpu[!0-3]", "cpu7"));
    }

    #[test]
    fn expands_patterns_below_an_explicit_sysfs_root() {
        let root = TempDir::new().unwrap();
        let sys = root.path().join("sys");
        let first = sys.join("devices/system/machinecheck/machinecheck0/ignore_ce");
        let second = sys.join("devices/system/machinecheck/machinecheck1/ignore_ce");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, "0").unwrap();
        fs::write(&second, "0").unwrap();

        let expanded = expand_pattern_under(
            "/sys/devices/system/machinecheck/machinecheck*/ignore_?e",
            &sys,
        )
        .unwrap();
        assert_eq!(expanded.len(), 2);
        assert!(expanded.contains(&first.canonicalize().unwrap()));
        assert!(expanded.contains(&second.canonicalize().unwrap()));
    }

    #[test]
    fn rejects_paths_outside_sysfs_and_relative_escape_components() {
        let root = TempDir::new().unwrap();
        let sys = root.path().join("sys");
        fs::create_dir_all(&sys).unwrap();
        assert!(expand_pattern_under("/etc/shadow", &sys).is_err());
        assert!(expand_pattern_under("/sys/../etc/shadow", &sys).is_err());
    }

    #[test]
    fn compares_active_choices_modifiers_and_numeric_encodings() {
        assert!(value_matches("performance|powersave", "[powersave] performance"));
        assert!(value_matches(">=1024", "2048"));
        assert!(!value_matches(">=1024", "512"));
        assert!(value_matches("0x0f", "15"));
    }
}
