use anyhow::{Context, Result};

pub type OrderedOptions = Vec<(String, String)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileUnit {
    pub name: String,
    pub plugin_type: String,
    pub priority: Option<i32>,
    pub enabled: bool,
    pub replace: bool,
    pub prepend: bool,
    pub drop: Vec<String>,
    pub devices: String,
    pub devices_udev_regex: Option<String>,
    pub cpuinfo_regex: Option<String>,
    pub uname_regex: Option<String>,
    pub script_pre: Option<String>,
    pub script_post: Option<String>,
    pub options: OrderedOptions,
}

impl ProfileUnit {
    pub fn from_options(name: &str, mut options: OrderedOptions) -> Result<Self> {
        let priority = take_option(&mut options, "priority")
            .map(|value| {
                value
                    .parse::<i32>()
                    .with_context(|| format!("Invalid priority '{value}' in unit '{name}'"))
            })
            .transpose()?;
        let plugin_type = take_option(&mut options, "type").unwrap_or_else(|| name.to_string());
        let enabled = take_option(&mut options, "enabled")
            .map(|value| tuned_bool(&value))
            .unwrap_or(true);
        let replace = take_option(&mut options, "replace")
            .map(|value| tuned_bool(&value))
            .unwrap_or(false);
        let prepend = take_option(&mut options, "prepend")
            .map(|value| tuned_bool(&value))
            .unwrap_or(false);
        let drop = take_option(&mut options, "drop")
            .map(|value| {
                value
                    .split([',', ';'])
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let devices = take_option(&mut options, "devices").unwrap_or_else(|| "*".to_string());
        let devices_udev_regex = take_option(&mut options, "devices_udev_regex");
        let cpuinfo_regex = take_option(&mut options, "cpuinfo_regex");
        let uname_regex = take_option(&mut options, "uname_regex");
        let script_pre = take_option(&mut options, "script_pre");
        let script_post = take_option(&mut options, "script_post");

        Ok(Self {
            name: name.to_string(),
            plugin_type,
            priority,
            enabled,
            replace,
            prepend,
            drop,
            devices,
            devices_udev_regex,
            cpuinfo_regex,
            uname_regex,
            script_pre,
            script_post,
            options,
        })
    }

    pub fn option(&self, name: &str) -> Option<&str> {
        option_value(&self.options, name)
    }

    pub fn merge_from(&mut self, mut newer: ProfileUnit) {
        self.plugin_type = newer.plugin_type;
        self.enabled = newer.enabled;
        self.devices = newer.devices;
        if newer.priority.is_some() {
            self.priority = newer.priority;
        }
        merge_optional(&mut self.devices_udev_regex, newer.devices_udev_regex);
        merge_optional(&mut self.cpuinfo_regex, newer.cpuinfo_regex);
        merge_optional(&mut self.uname_regex, newer.uname_regex);
        merge_optional(&mut self.script_pre, newer.script_pre);
        merge_optional(&mut self.script_post, newer.script_post);

        for option in newer.drop.drain(..) {
            remove_option(&mut self.options, &option);
        }

        if self.name == "script" {
            if let Some(incoming) = take_option(&mut newer.options, "script") {
                match self.options.iter_mut().find(|(name, _)| name == "script") {
                    Some((_, current)) if !current.is_empty() => {
                        current.push('\n');
                        current.push_str(&incoming);
                    }
                    Some((_, current)) => *current = incoming,
                    None => self.options.push(("script".to_string(), incoming)),
                }
            }
        }

        merge_options(&mut self.options, newer.options);
        self.replace = false;
        self.prepend = false;
        self.drop.clear();
    }
}

pub fn merge_units(current: &mut Vec<ProfileUnit>, newer: Vec<ProfileUnit>) {
    for unit in newer {
        match current
            .iter()
            .position(|existing| existing.name == unit.name)
        {
            Some(index) if unit.replace => current[index] = unit,
            Some(index) => current[index].merge_from(unit),
            None => current.push(unit),
        }
    }
}

pub fn merge_variables(
    current: &mut OrderedOptions,
    newer: OrderedOptions,
    replace: bool,
    prepend: bool,
) {
    if replace {
        current.clear();
    }

    let mut added = Vec::new();
    for (name, value) in newer {
        if let Some((_, existing)) = current
            .iter_mut()
            .find(|(existing_name, _)| existing_name == &name)
        {
            *existing = value;
        } else {
            added.push((name, value));
        }
    }

    if prepend {
        added.extend(std::mem::take(current));
        *current = added;
    } else {
        current.extend(added);
    }
}

pub fn merge_options(current: &mut OrderedOptions, newer: OrderedOptions) {
    for (name, value) in newer {
        set_option(current, name, value);
    }
}

pub fn option_value<'a>(options: &'a OrderedOptions, name: &str) -> Option<&'a str> {
    options
        .iter()
        .find_map(|(option, value)| (option == name).then_some(value.as_str()))
}

pub fn set_option(options: &mut OrderedOptions, name: String, value: String) {
    if let Some((_, current)) = options
        .iter_mut()
        .find(|(current_name, _)| current_name == &name)
    {
        *current = value;
    } else {
        options.push((name, value));
    }
}

fn take_option(options: &mut OrderedOptions, name: &str) -> Option<String> {
    options
        .iter()
        .position(|(option, _)| option == name)
        .map(|index| options.remove(index).1)
}

fn remove_option(options: &mut OrderedOptions, name: &str) {
    if let Some(index) = options.iter().position(|(option, _)| option == name) {
        options.remove(index);
    }
}

fn merge_optional<T>(current: &mut Option<T>, newer: Option<T>) {
    if newer.is_some() {
        *current = newer;
    }
}

fn tuned_bool(value: &str) -> bool {
    matches!(value, "True" | "true" | "1")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(values: &[(&str, &str)]) -> OrderedOptions {
        values
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn parses_unit_controls_without_exposing_them_as_plugin_options() {
        let unit = ProfileUnit::from_options(
            "cpu.fast",
            options(&[
                ("type", "cpu"),
                ("priority", "20"),
                ("enabled", "true"),
                ("replace", "1"),
                ("devices", "cpu0,cpu1"),
                ("drop", "boost; governor"),
                ("governor", "performance"),
            ]),
        )
        .unwrap();

        assert_eq!(unit.name, "cpu.fast");
        assert_eq!(unit.plugin_type, "cpu");
        assert_eq!(unit.priority, Some(20));
        assert!(unit.enabled);
        assert!(unit.replace);
        assert_eq!(unit.devices, "cpu0,cpu1");
        assert_eq!(unit.drop, ["boost", "governor"]);
        assert_eq!(unit.option("governor"), Some("performance"));
        assert_eq!(unit.option("priority"), None);
    }

    #[test]
    fn overlays_metadata_drops_options_and_preserves_unspecified_fields() {
        let mut current = ProfileUnit::from_options(
            "cpu",
            options(&[
                ("priority", "10"),
                ("cpuinfo_regex", "old"),
                ("governor", "powersave"),
                ("boost", "0"),
            ]),
        )
        .unwrap();
        let newer = ProfileUnit::from_options(
            "cpu",
            options(&[
                ("drop", "boost"),
                ("devices", "cpu0"),
                ("governor", "performance"),
            ]),
        )
        .unwrap();

        current.merge_from(newer);
        assert_eq!(current.priority, Some(10));
        assert_eq!(current.cpuinfo_regex.as_deref(), Some("old"));
        assert_eq!(current.devices, "cpu0");
        assert_eq!(current.option("governor"), Some("performance"));
        assert_eq!(current.option("boost"), None);
    }

    #[test]
    fn replace_discards_the_previous_unit() {
        let first =
            ProfileUnit::from_options("sysctl", options(&[("vm.swappiness", "60")])).unwrap();
        let replacement = ProfileUnit::from_options(
            "sysctl",
            options(&[("replace", "true"), ("kernel.nmi_watchdog", "0")]),
        )
        .unwrap();
        let mut units = vec![first];

        merge_units(&mut units, vec![replacement]);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].option("vm.swappiness"), None);
        assert_eq!(units[0].option("kernel.nmi_watchdog"), Some("0"));
    }

    #[test]
    fn script_units_append_scripts_in_profile_order() {
        let mut first =
            ProfileUnit::from_options("script", options(&[("script", "/one.sh")])).unwrap();
        let second =
            ProfileUnit::from_options("script", options(&[("script", "/two.sh")])).unwrap();

        first.merge_from(second);
        assert_eq!(first.option("script"), Some("/one.sh\n/two.sh"));
    }

    #[test]
    fn prepended_variables_keep_their_declared_order() {
        let mut variables = options(&[("base", "one"), ("shared", "old")]);
        merge_variables(
            &mut variables,
            options(&[("first", "a"), ("second", "b"), ("shared", "new")]),
            false,
            true,
        );
        assert_eq!(
            variables,
            options(&[
                ("first", "a"),
                ("second", "b"),
                ("base", "one"),
                ("shared", "new"),
            ])
        );
    }
}
