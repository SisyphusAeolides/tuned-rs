use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use tracing::debug;

use crate::config;
use crate::profile::Profile;
use crate::profile_units::{OrderedOptions, ProfileUnit};

#[derive(Debug, Clone, Default)]
pub struct VariableSet {
    values: BTreeMap<String, String>,
}

impl VariableSet {
    pub fn from_profile(profile: &Profile) -> Result<Self> {
        let mut variables = Self::default();
        for (name, raw) in &profile.variables {
            if name == "include" {
                let path = variables.expand(raw)?;
                variables.add_file(&path)?;
            } else {
                variables.add(name, raw)?;
            }
        }
        Ok(variables)
    }

    pub fn environment(&self) -> BTreeMap<String, String> {
        self.values
            .iter()
            .map(|(name, value)| (format!("TUNED_{name}"), value.clone()))
            .collect()
    }

    pub fn expand(&self, raw: &str) -> Result<String> {
        expand_with(raw, &self.values)
    }

    fn add(&mut self, name: &str, raw: &str) -> Result<()> {
        validate_variable_name(name)?;
        let value = self.expand(raw)?;
        self.values.insert(name.to_string(), value);
        Ok(())
    }

    fn add_file(&mut self, raw_path: &str) -> Result<()> {
        let path = config::resolve_path_buf(raw_path);
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read variables file {}", path.display()))?;
        for (line_number, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                bail!(
                    "Invalid variable definition at {}:{}",
                    path.display(),
                    line_number + 1
                );
            };
            self.add(name.trim(), value.trim())?;
        }
        Ok(())
    }
}

pub fn active_units(profile: &Profile) -> Result<Vec<ProfileUnit>> {
    let variables = VariableSet::from_profile(profile)?;
    let needs_cpuinfo = profile
        .units
        .iter()
        .any(|unit| unit.enabled && unit.cpuinfo_regex.is_some());
    let needs_uname = profile
        .units
        .iter()
        .any(|unit| unit.enabled && unit.uname_regex.is_some());
    let cpuinfo = needs_cpuinfo
        .then(|| configured_or_file("cpuinfo_string", "/proc/cpuinfo", "TUNED_RS_CPUINFO_STRING"))
        .transpose()?;
    let uname = needs_uname.then(configured_uname).transpose()?;

    let mut active = Vec::new();
    for (index, unit) in profile.units.iter().enumerate() {
        if !unit.enabled {
            continue;
        }
        let unit = expand_unit(unit.clone(), &variables)?;
        if let Some(pattern) = unit.cpuinfo_regex.as_deref() {
            if !regex_search(pattern, cpuinfo.as_deref().unwrap_or_default())? {
                debug!(
                    "Skipping unit '{}' because cpuinfo does not match",
                    unit.name
                );
                continue;
            }
        }
        if let Some(pattern) = unit.uname_regex.as_deref() {
            if !regex_search(pattern, uname.as_deref().unwrap_or_default())? {
                debug!("Skipping unit '{}' because uname does not match", unit.name);
                continue;
            }
        }
        active.push((unit.priority.unwrap_or(0), index, unit));
    }

    active.sort_by_key(|(priority, index, _)| (*priority, *index));
    Ok(active.into_iter().map(|(_, _, unit)| unit).collect())
}

pub fn variable_environment(profile: &Profile) -> Result<BTreeMap<String, String>> {
    Ok(VariableSet::from_profile(profile)?.environment())
}

fn expand_unit(mut unit: ProfileUnit, variables: &VariableSet) -> Result<ProfileUnit> {
    unit.plugin_type = variables.expand(&unit.plugin_type)?;
    unit.devices = variables.expand(&unit.devices)?;
    expand_optional(&mut unit.devices_udev_regex, variables)?;
    expand_optional(&mut unit.cpuinfo_regex, variables)?;
    expand_optional(&mut unit.uname_regex, variables)?;
    expand_optional(&mut unit.script_pre, variables)?;
    expand_optional(&mut unit.script_post, variables)?;
    unit.options = expand_options(unit.options, variables)?;
    Ok(unit)
}

fn expand_options(options: OrderedOptions, variables: &VariableSet) -> Result<OrderedOptions> {
    options
        .into_iter()
        .map(|(name, value)| Ok((variables.expand(&name)?, variables.expand(&value)?)))
        .collect()
}

fn expand_optional(value: &mut Option<String>, variables: &VariableSet) -> Result<()> {
    if let Some(raw) = value.take() {
        *value = Some(variables.expand(&raw)?);
    }
    Ok(())
}

fn expand_with(raw: &str, values: &BTreeMap<String, String>) -> Result<String> {
    const ESCAPED: &str = "\u{1e}TUNED_ESCAPED_VARIABLE\u{1e}";
    const DOUBLED: &str = "\u{1e}TUNED_DOUBLED_DOLLAR\u{1e}";

    let mut current = raw
        .replace("\\${", &format!("{ESCAPED}{{"))
        .replace("$${", &format!("{DOUBLED}{{"));

    for _ in 0..64 {
        let (next, changed) = expand_once(&current, values)?;
        current = next;
        if !changed {
            return Ok(current
                .replace(&format!("{ESCAPED}{{"), "${")
                .replace(&format!("{DOUBLED}{{"), "${"));
        }
    }
    bail!("Variable expansion exceeded the recursion limit in '{raw}'")
}

fn expand_once(raw: &str, values: &BTreeMap<String, String>) -> Result<(String, bool)> {
    let mut output = String::with_capacity(raw.len());
    let mut cursor = 0usize;
    let mut changed = false;

    while let Some(relative) = raw[cursor..].find("${") {
        let start = cursor + relative;
        output.push_str(&raw[cursor..start]);
        let Some(relative_end) = raw[start + 2..].find('}') else {
            output.push_str(&raw[start..]);
            return Ok((output, changed));
        };
        let end = start + 2 + relative_end;
        let name = &raw[start + 2..end];
        if name.starts_with("i:") || name.starts_with("f:") {
            output.push_str(&raw[start..=end]);
        } else if let Some(value) = values.get(name) {
            output.push_str(value);
            changed = true;
        } else {
            output.push_str(&raw[start..=end]);
        }
        cursor = end + 1;
    }
    output.push_str(&raw[cursor..]);
    Ok((output, changed))
}

fn validate_variable_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        bail!("Invalid TuneD variable name '{name}'");
    }
    Ok(())
}

fn configured_or_file(key: &str, fallback: &str, environment: &str) -> Result<String> {
    if let Ok(value) = std::env::var(environment) {
        return Ok(value);
    }
    if let Some(value) = configured_value(key)? {
        return Ok(value);
    }
    fs::read_to_string(config::resolve_path(fallback))
        .with_context(|| format!("Failed to read {fallback}"))
}

fn configured_uname() -> Result<String> {
    if let Ok(value) = std::env::var("TUNED_RS_UNAME_STRING") {
        return Ok(value);
    }
    if let Some(value) = configured_value("uname_string")? {
        return Ok(value);
    }
    let output = Command::new("uname")
        .arg("-a")
        .output()
        .context("Failed to execute uname")?;
    if !output.status.success() {
        bail!("uname exited with {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn configured_value(key: &str) -> Result<Option<String>> {
    let path = config::resolve_path(config::GLOBAL_CONFIG_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let mut ini = configparser::ini::Ini::new();
    ini.load(path.to_str().unwrap_or_default())
        .map_err(|error| anyhow::anyhow!("Failed to parse {}: {error}", path.display()))?;
    Ok(ini.get("main", key).filter(|value| !value.is_empty()))
}

fn regex_search(pattern: &str, text: &str) -> Result<bool> {
    if pattern.is_empty() {
        return Ok(true);
    }
    if pattern.len() > 4096 || pattern.contains('\0') {
        bail!("Invalid profile condition regular expression");
    }
    let mut child = Command::new("grep")
        .args(["-P", "-q", "-e", pattern])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to execute grep for profile condition matching")?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("grep stdin was unavailable"))?
        .write_all(text.as_bytes())?;
    let output = child.wait_with_output()?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "Invalid profile condition regex '{}': {}",
            pattern,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile_units::ProfileUnit;

    #[test]
    fn expands_variables_sequentially_and_preserves_escaped_references() {
        let mut profile = Profile::default();
        profile.variables = vec![
            ("ROOT".to_string(), "/srv".to_string()),
            ("FILE".to_string(), "${ROOT}/data".to_string()),
        ];
        let variables = VariableSet::from_profile(&profile).unwrap();
        assert_eq!(variables.expand("${FILE}").unwrap(), "/srv/data");
        assert_eq!(variables.expand("\\${FILE}").unwrap(), "${FILE}");
        assert_eq!(variables.expand("$${FILE}").unwrap(), "${FILE}");
        assert_eq!(variables.environment()["TUNED_FILE"], "/srv/data");
    }

    #[test]
    fn filters_conditions_and_sorts_priority_stably() {
        std::env::set_var("TUNED_RS_CPUINFO_STRING", "CPU part : 0x516\n");
        std::env::set_var("TUNED_RS_UNAME_STRING", "Linux host 6.15 aarch64");
        let mut profile = Profile::default();
        profile.units = vec![
            ProfileUnit::from_options("late", vec![("priority".to_string(), "20".to_string())])
                .unwrap(),
            ProfileUnit::from_options(
                "matching",
                vec![
                    ("priority".to_string(), "10".to_string()),
                    ("uname_regex".to_string(), "aarch64".to_string()),
                    (
                        "cpuinfo_regex".to_string(),
                        r"CPU part\s*:\s*0x516".to_string(),
                    ),
                ],
            )
            .unwrap(),
            ProfileUnit::from_options(
                "skipped",
                vec![("uname_regex".to_string(), "x86_64".to_string())],
            )
            .unwrap(),
        ];

        let units = active_units(&profile).unwrap();
        assert_eq!(
            units
                .iter()
                .map(|unit| unit.name.as_str())
                .collect::<Vec<_>>(),
            vec!["matching", "late"]
        );
        std::env::remove_var("TUNED_RS_CPUINFO_STRING");
        std::env::remove_var("TUNED_RS_UNAME_STRING");
    }

    #[test]
    fn regex_engine_rejects_invalid_patterns() {
        assert!(regex_search("a+", "baa").unwrap());
        assert!(!regex_search("^z", "baa").unwrap());
        assert!(regex_search("(", "anything").is_err());
    }
}
