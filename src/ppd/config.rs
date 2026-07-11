use std::collections::{HashMap, HashSet};

use anyhow::{bail, Context, Result};
use configparser::ini::Ini;

pub const CONFIG_FILE: &str = "/etc/tuned/ppd.conf";
pub const BASE_PROFILE_FILE: &str = "/etc/tuned/ppd_base_profile";
pub const API_VERSION: &str = "0.23";
pub const DRIVER: &str = "tuned";

pub const POWER_SAVER: &str = "power-saver";
pub const BALANCED: &str = "balanced";
pub const PERFORMANCE: &str = "performance";
pub const UNKNOWN_PROFILE: &str = "unknown";

pub const UPOWER_BUS: &str = "org.freedesktop.UPower.PowerProfiles";
pub const UPOWER_PATH: &str = "/org/freedesktop/UPower/PowerProfiles";
pub const HADESS_BUS: &str = "net.hadess.PowerProfiles";
pub const HADESS_PATH: &str = "/net/hadess/PowerProfiles";

pub const UPOWER_POWER_BUS: &str = "org.freedesktop.UPower";
pub const UPOWER_POWER_PATH: &str = "/org/freedesktop/UPower";

pub const NO_TURBO_PATH: &str = "/sys/devices/system/cpu/intel_pstate/no_turbo";
pub const LAP_MODE_PATH: &str = "/sys/bus/platform/devices/thinkpad_acpi/dytc_lapmode";
pub const PLATFORM_PROFILE_PATH: &str = "/sys/firmware/acpi/platform_profile";

#[derive(Debug, Clone)]
pub struct ProfileMap {
    ac: HashMap<String, String>,
    dc: HashMap<String, String>,
}

impl ProfileMap {
    pub fn get(&self, profile: &str, on_battery: bool) -> Result<&str> {
        let map = if on_battery { &self.dc } else { &self.ac };
        map.get(profile)
            .map(String::as_str)
            .with_context(|| format!("Unknown profile '{profile}'"))
    }

    pub fn keys(&self, on_battery: bool) -> impl Iterator<Item = &String> {
        let map = if on_battery { &self.dc } else { &self.ac };
        map.keys()
    }

    pub fn reverse_lookup(&self, tuned_profile: &str, on_battery: bool) -> Option<&str> {
        let (primary, fallback) = if on_battery {
            (&self.dc, &self.ac)
        } else {
            (&self.ac, &self.dc)
        };
        primary
            .get(tuned_profile)
            .or_else(|| fallback.get(tuned_profile))
            .map(String::as_str)
    }
}

#[derive(Debug, Clone)]
pub struct PpdConfig {
    pub default_profile: String,
    pub battery_detection: bool,
    pub sysfs_acpi_monitor: bool,
    pub ppd_to_tuned: ProfileMap,
    pub tuned_to_ppd: ProfileMap,
}

impl PpdConfig {
    pub fn load(path: &str, tuned_profiles: &[String]) -> Result<Self> {
        if !std::path::Path::new(path).is_file() {
            bail!("Configuration file '{path}' does not exist");
        }

        let mut ini = Ini::new();
        ini.load(path)
            .map_err(|error| anyhow::anyhow!("Error parsing configuration file '{path}': {error}"))?;

        let profile_dict_ac = section_map(&ini, "profiles")
            .ok_or_else(|| anyhow::anyhow!("Missing profiles section in '{path}'"))?;

        for required in [POWER_SAVER, BALANCED, PERFORMANCE] {
            if !profile_dict_ac.contains_key(required) {
                bail!("Missing {required} profile in '{path}'");
            }
        }

        let default_profile = ini
            .get("main", "default")
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| BALANCED.to_string());

        if !profile_dict_ac.contains_key(&default_profile) {
            bail!("Default profile '{default_profile}' missing in profile mapping");
        }

        let battery_section = section_map(&ini, "battery");
        let battery_detection = ini
            .get("main", "battery_detection")
            .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(battery_section.is_some());

        if battery_detection && battery_section.is_none() {
            bail!("Missing battery section in '{path}'");
        }

        let profile_dict_dc = if let Some(battery) = battery_section {
            profile_dict_ac
                .iter()
                .chain(battery.iter())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        } else {
            profile_dict_ac.clone()
        };

        let tuned_set: HashSet<_> = tuned_profiles.iter().cloned().collect();
        let unknown: Vec<_> = profile_dict_ac
            .values()
            .chain(profile_dict_dc.values())
            .filter(|profile| !tuned_set.contains(*profile))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            bail!(
                "Unknown TuneD profiles in configuration file: {}",
                unknown.join(", ")
            );
        }

        let unknown_battery: Vec<_> = profile_dict_dc
            .keys()
            .filter(|profile| !profile_dict_ac.contains_key(*profile))
            .cloned()
            .collect();
        if !unknown_battery.is_empty() {
            bail!(
                "Unknown PPD profiles in battery section: {}",
                unknown_battery.join(", ")
            );
        }

        if profile_dict_ac.values().collect::<HashSet<_>>().len() != profile_dict_ac.len()
            || profile_dict_dc.values().collect::<HashSet<_>>().len() != profile_dict_dc.len()
        {
            bail!("Duplicate profile mapping in '{path}'");
        }

        let sysfs_acpi_monitor = ini
            .get("main", "sysfs_acpi_monitor")
            .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(true);

        let tuned_to_ac: HashMap<String, String> = profile_dict_ac
            .iter()
            .map(|(ppd, tuned)| (tuned.clone(), ppd.clone()))
            .collect();
        let tuned_to_dc: HashMap<String, String> = profile_dict_dc
            .iter()
            .map(|(ppd, tuned)| (tuned.clone(), ppd.clone()))
            .collect();

        Ok(Self {
            default_profile,
            battery_detection,
            sysfs_acpi_monitor,
            ppd_to_tuned: ProfileMap {
                ac: profile_dict_ac,
                dc: profile_dict_dc,
            },
            tuned_to_ppd: ProfileMap {
                ac: tuned_to_ac,
                dc: tuned_to_dc,
            },
        })
    }
}

fn section_map(ini: &Ini, section: &str) -> Option<HashMap<String, String>> {
    let section = ini.get_map_ref().get(section)?;
    Some(
        section
            .iter()
            .filter_map(|(key, value)| value.as_ref().map(|value| (key.clone(), value.clone())))
            .collect(),
    )
}

pub fn platform_profile_to_ppd(raw: &str) -> Option<&'static str> {
    match raw.trim() {
        "low-power" => Some(POWER_SAVER),
        "balanced" => Some(BALANCED),
        "performance" => Some(PERFORMANCE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn loads_valid_config() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            "[main]\n\
             default=balanced\n\
             battery_detection=false\n\
             [profiles]\n\
             power-saver=powersave\n\
             balanced=balanced\n\
             performance=latency-performance\n"
        )
        .unwrap();

        let profiles = vec![
            "powersave".to_string(),
            "balanced".to_string(),
            "latency-performance".to_string(),
        ];
        let config = PpdConfig::load(file.path().to_str().unwrap(), &profiles).unwrap();
        assert_eq!(config.default_profile, "balanced");
        assert_eq!(
            config.ppd_to_tuned.get("balanced", false).unwrap(),
            "balanced"
        );
        assert_eq!(
            config
                .tuned_to_ppd
                .reverse_lookup("latency-performance", false)
                .unwrap(),
            "performance"
        );
        assert_eq!(
            config
                .tuned_to_ppd
                .reverse_lookup("balanced", false)
                .unwrap(),
            "balanced"
        );
    }

    #[test]
    fn reverse_lookup_uses_battery_map_on_dc() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            "[main]\n\
             default=balanced\n\
             [profiles]\n\
             power-saver=powersave\n\
             balanced=throughput-performance\n\
             performance=latency-performance\n\
             [battery]\n\
             balanced=balanced-battery\n"
        )
        .unwrap();

        let profiles = vec![
            "powersave".to_string(),
            "throughput-performance".to_string(),
            "latency-performance".to_string(),
            "balanced-battery".to_string(),
        ];
        let config = PpdConfig::load(file.path().to_str().unwrap(), &profiles).unwrap();
        assert_eq!(
            config
                .tuned_to_ppd
                .reverse_lookup("balanced-battery", true)
                .unwrap(),
            "balanced"
        );
        assert_eq!(
            config
                .tuned_to_ppd
                .reverse_lookup("throughput-performance", true)
                .unwrap(),
            "balanced"
        );
    }
}
