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
        expand_with(raw, &self.values, true)
    }

    fn add(&mut self, name: &str, raw: &str) -> Result<()> {
        validate_variable_name(name)?;
        if raw.trim() == format!("${{{name}}}") {
            self.values.entry(name.to_string()).or_default();
            return Ok(());
        }
        let value = expand_with(raw, &self.values, false)?;
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
        active.push((
            unit.priority
                .unwrap_or_else(crate::config::default_instance_priority),
            index,
            unit,
        ));
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

fn expand_with(raw: &str, values: &BTreeMap<String, String>, finalize: bool) -> Result<String> {
    const ESCAPED: &str = "\u{1e}TUNED_ESCAPED_VARIABLE\u{1e}";
    const DOUBLED: &str = "\u{1e}TUNED_DOUBLED_DOLLAR\u{1e}";
    const UNKNOWN: &str = "\u{1e}TUNED_UNKNOWN_VARIABLE\u{1e}";
    const LITERAL_END: &str = "\u{1d}TUNED_LITERAL_END\u{1d}";

    let current = encode_literal_expressions(raw, "\\${", ESCAPED, LITERAL_END)?;
    let mut current = encode_literal_expressions(&current, "$${", DOUBLED, LITERAL_END)?;

    for _ in 0..128 {
        let Some(first_start) = current.find("${") else {
            return Ok(if finalize {
                current
                    .replace(ESCAPED, "${")
                    .replace(DOUBLED, "${")
                    .replace(UNKNOWN, "${")
                    .replace(LITERAL_END, "}")
            } else {
                current
            });
        };
        let Some(relative_end) = current[first_start..].find('}') else {
            bail!("Unterminated variable or function expression in '{raw}'");
        };
        let end = first_start + relative_end;
        let Some(start) = current[..end].rfind("${") else {
            bail!("Unmatched closing brace in '{raw}'");
        };
        let expression = &current[start + 2..end];
        let replacement = if let Some(function) = expression.strip_prefix("f:") {
            crate::profile_functions::evaluate(function)?
        } else if expression.starts_with("i:") {
            format!("{UNKNOWN}{expression}{LITERAL_END}")
        } else {
            values.get(expression).cloned().unwrap_or_default()
        };
        current.replace_range(start..=end, &replacement);
    }
    bail!("Variable and function expansion exceeded the recursion limit in '{raw}'")
}

fn encode_literal_expressions(
    raw: &str,
    prefix: &str,
    marker: &str,
    end_marker: &str,
) -> Result<String> {
    let mut output = raw.to_string();
    let mut cursor = 0;
    while let Some(relative_start) = output[cursor..].find(prefix) {
        let start = cursor + relative_start;
        let body_start = start + prefix.len();
        let relative_end = output[body_start..]
            .find('}')
            .ok_or_else(|| anyhow::anyhow!("Unterminated escaped TuneD expression in '{raw}'"))?;
        let end = body_start + relative_end;
        let body = output[body_start..end].to_string();
        let replacement = format!("{marker}{body}{end_marker}");
        output.replace_range(start..=end, &replacement);
        cursor = start + replacement.len();
    }
    Ok(output)
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
    use std::fs;

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
        assert_eq!(
            variables.expand("${f:strip:  ${FILE}  }").unwrap(),
            "/srv/data"
        );
        assert_eq!(variables.environment()["TUNED_FILE"], "/srv/data");
    }

    #[test]
    fn self_assignment_preserves_an_external_value_or_defaults_empty() {
        let mut variables = VariableSet::default();
        variables.add("isolated_cores", "2-7").unwrap();
        variables
            .add("isolated_cores", "${isolated_cores}")
            .unwrap();
        variables
            .add("no_balance_cores", "${no_balance_cores}")
            .unwrap();
        variables
            .add("missing_marker", "\\${no_balance_cores}")
            .unwrap();

        assert_eq!(variables.expand("${isolated_cores}").unwrap(), "2-7");
        assert_eq!(variables.expand("${no_balance_cores}").unwrap(), "");
        assert_eq!(
            variables.expand("${missing_marker}").unwrap(),
            "${no_balance_cores}"
        );
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

    #[test]
    fn resolves_external_variables_and_nested_profile_functions() {
        let _lock = crate::config::test_env_lock();
        let root = tempfile::tempdir().unwrap();
        for cpu in 0..4 {
            let topology = root
                .path()
                .join(format!("sys/devices/system/cpu/cpu{cpu}/topology"));
            fs::create_dir_all(&topology).unwrap();
            fs::write(topology.join("physical_package_id"), "0\n").unwrap();
        }
        fs::create_dir_all(root.path().join("sys/devices/system/cpu")).unwrap();
        fs::write(root.path().join("sys/devices/system/cpu/online"), "0-3\n").unwrap();
        fs::write(root.path().join("sys/devices/system/cpu/present"), "0-3\n").unwrap();
        fs::create_dir_all(root.path().join("etc/tuned")).unwrap();
        fs::write(
            root.path().join("etc/tuned/realtime-variables.conf"),
            "isolated_cores=${f:calc_isolated_cores:1}\n",
        )
        .unwrap();

        let mut profile = Profile::default();
        profile.variables = vec![
            (
                "include".to_string(),
                "/etc/tuned/realtime-variables.conf".to_string(),
            ),
            (
                "isolated_cores_expanded".to_string(),
                "${f:cpulist_unpack:${isolated_cores}}".to_string(),
            ),
            (
                "isolated_cores_online".to_string(),
                "${f:cpulist_online:${isolated_cores}}".to_string(),
            ),
            (
                "assert_online".to_string(),
                "${f:assertion:isolated CPUs are online:${isolated_cores_expanded}:${isolated_cores_online}}"
                    .to_string(),
            ),
        ];
        profile.units = vec![ProfileUnit::from_options(
            "sysctl",
            vec![(
                "kernel.test_mask".to_string(),
                "${f:cpulist2hex:${isolated_cores_expanded}}".to_string(),
            )],
        )
        .unwrap()];

        std::env::set_var("TUNED_RS_ROOT", root.path());
        let units = active_units(&profile).unwrap();
        std::env::remove_var("TUNED_RS_ROOT");

        assert_eq!(units[0].options[0].1, "0000000e");
    }
}
