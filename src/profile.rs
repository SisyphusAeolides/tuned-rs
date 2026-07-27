use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use configparser::ini::Ini;
use tracing::{info, warn};

use crate::config::{self, PROFILE_FILE};
use crate::engine;

#[derive(Debug, Clone, Default)]
pub struct CpuSettings {
    pub governor: Option<String>,
    pub energy_performance_preference: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct VmSettings {
    pub transparent_hugepages: Option<String>,
    pub transparent_hugepage_defrag: Option<String>,
    pub dirty_bytes: Option<String>,
    pub dirty_ratio: Option<String>,
    pub dirty_background_bytes: Option<String>,
    pub dirty_background_ratio: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DiskSettings {
    pub devices: Option<String>,
    pub elevator: Option<String>,
    pub readahead: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct NetworkSettings {
    pub tcp_congestion_control: Option<String>,
    pub tcp_window_scaling: Option<String>,
    pub tcp_timestamps: Option<String>,
    pub tcp_sack: Option<String>,
    pub tcp_fastopen: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AcpiSettings {
    pub platform_profile: Option<String>,
}

pub type PluginOptions = Vec<(String, String)>;

#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub name: String,
    pub summary: String,
    pub description: String,
    pub cpu: CpuSettings,
    pub vm: VmSettings,
    pub disk: DiskSettings,
    pub acpi: AcpiSettings,
    pub network: NetworkSettings,
    pub sysctl: HashMap<String, String>,
    pub gpu: PluginOptions,
    pub storage: PluginOptions,
    pub thermal: PluginOptions,
    pub battery: PluginOptions,
    pub hermes: PluginOptions,
}

impl Profile {
    fn merge_from(&mut self, newer: Profile) {
        if !newer.summary.is_empty() {
            self.summary = newer.summary;
        }
        if !newer.description.is_empty() {
            self.description = newer.description;
        }

        merge_optional(&mut self.cpu.governor, newer.cpu.governor);
        merge_optional(
            &mut self.cpu.energy_performance_preference,
            newer.cpu.energy_performance_preference,
        );

        merge_optional(
            &mut self.vm.transparent_hugepages,
            newer.vm.transparent_hugepages,
        );
        merge_optional(
            &mut self.vm.transparent_hugepage_defrag,
            newer.vm.transparent_hugepage_defrag,
        );
        merge_optional(&mut self.vm.dirty_bytes, newer.vm.dirty_bytes);
        merge_optional(&mut self.vm.dirty_ratio, newer.vm.dirty_ratio);
        merge_optional(
            &mut self.vm.dirty_background_bytes,
            newer.vm.dirty_background_bytes,
        );
        merge_optional(
            &mut self.vm.dirty_background_ratio,
            newer.vm.dirty_background_ratio,
        );

        merge_optional(&mut self.disk.devices, newer.disk.devices);
        merge_optional(&mut self.disk.elevator, newer.disk.elevator);
        merge_optional(&mut self.disk.readahead, newer.disk.readahead);
        merge_optional(&mut self.acpi.platform_profile, newer.acpi.platform_profile);

        merge_optional(
            &mut self.network.tcp_congestion_control,
            newer.network.tcp_congestion_control,
        );
        merge_optional(
            &mut self.network.tcp_window_scaling,
            newer.network.tcp_window_scaling,
        );
        merge_optional(
            &mut self.network.tcp_timestamps,
            newer.network.tcp_timestamps,
        );
        merge_optional(&mut self.network.tcp_sack, newer.network.tcp_sack);
        merge_optional(&mut self.network.tcp_fastopen, newer.network.tcp_fastopen);

        self.sysctl.extend(newer.sysctl);
        merge_plugin_options(&mut self.gpu, newer.gpu);
        merge_plugin_options(&mut self.storage, newer.storage);
        merge_plugin_options(&mut self.thermal, newer.thermal);
        merge_plugin_options(&mut self.battery, newer.battery);
        merge_plugin_options(&mut self.hermes, newer.hermes);
    }
}

#[derive(Debug, Clone)]
pub struct ProfileCatalog {
    profiles: HashMap<String, Profile>,
}

impl ProfileCatalog {
    pub fn load_from_dirs(dirs: &[PathBuf]) -> Result<Self> {
        let sources = collect_profile_sources(dirs)?;
        let mut names = sources.keys().cloned().collect::<Vec<_>>();
        names.sort_unstable();

        let mut profiles = HashMap::new();
        for name in names {
            let mut layers = Vec::new();
            let mut processed = HashSet::new();
            match load_profile_layers(&name, &sources, &mut processed, &mut layers) {
                Ok(()) => {
                    let mut profile = Profile::default();
                    for layer in layers {
                        profile.merge_from(layer);
                    }
                    profile.name = name.clone();
                    profiles.insert(name, profile);
                }
                Err(error) => warn!("Skipping profile '{name}': {error}"),
            }
        }

        info!("Loaded {} TuneD profile(s)", profiles.len());
        Ok(Self { profiles })
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.profiles.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    pub fn summaries(&self) -> Vec<(String, String)> {
        let mut entries: Vec<_> = self
            .profiles
            .values()
            .map(|profile| (profile.name.clone(), profile.summary.clone()))
            .collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        entries
    }

    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }

    pub fn resolve(&self, selection: &str) -> Option<Profile> {
        let names = engine::profile_selection(selection)?;
        let normalized = names.join(" ");
        let mut profile = Profile::default();
        for name in names {
            profile.merge_from(self.profiles.get(name)?.clone());
        }
        profile.name = normalized;
        Some(profile)
    }

    pub fn profile_info(&self, name: &str) -> (bool, String, String, String) {
        match self.profiles.get(name) {
            Some(profile) => (
                true,
                profile.name.clone(),
                profile.summary.clone(),
                profile.description.clone(),
            ),
            None => (false, String::new(), String::new(), String::new()),
        }
    }

    pub fn recommend(&self) -> String {
        if self.profiles.contains_key(config::DEFAULT_PROFILE) {
            config::DEFAULT_PROFILE.to_string()
        } else {
            self.names()
                .into_iter()
                .next()
                .unwrap_or_else(|| config::DEFAULT_PROFILE.to_string())
        }
    }
}

fn collect_profile_sources(dirs: &[PathBuf]) -> Result<HashMap<String, PathBuf>> {
    let mut sources = HashMap::new();

    for dir in dirs {
        if !dir.is_dir() {
            warn!("Profile directory does not exist: {}", dir.display());
            continue;
        }

        for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let name = entry.file_name().to_string_lossy().into_owned();
            if !engine::validate_profile_name(&name) {
                continue;
            }

            let path = entry.path().join(PROFILE_FILE);
            if path.is_file() {
                sources.insert(name, path);
            }
        }
    }

    Ok(sources)
}

fn load_profile_layers(
    requested_name: &str,
    sources: &HashMap<String, PathBuf>,
    processed: &mut HashSet<PathBuf>,
    layers: &mut Vec<Profile>,
) -> Result<()> {
    let (conditional, name) = requested_name
        .strip_prefix('-')
        .map_or((false, requested_name), |name| (true, name));

    if !engine::validate_profile_name(name) {
        bail!("Invalid profile name '{requested_name}'");
    }

    let Some(path) = sources.get(name) else {
        if conditional {
            return Ok(());
        }
        bail!("Profile '{name}' not found");
    };

    if !processed.insert(path.clone()) {
        return Ok(());
    }

    for include in read_includes(path)? {
        load_profile_layers(&include, sources, processed, layers)?;
    }
    layers.push(load_profile(path, name)?);
    Ok(())
}

fn read_includes(path: &Path) -> Result<Vec<String>> {
    let ini = read_ini(path)?;
    let profile_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let include = ini
        .get("main", "include")
        .map(|value| expand_profile_dir(&value, profile_dir))
        .unwrap_or_default();

    Ok(include
        .split([',', ';'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect())
}

pub fn load_profile(path: &Path, name: &str) -> Result<Profile> {
    let ini = read_ini(path)?;
    let profile_dir = path.parent().unwrap_or_else(|| Path::new("."));

    let summary = section_value(&ini, profile_dir, "main", "summary").unwrap_or_default();
    let description = section_value(&ini, profile_dir, "main", "description").unwrap_or_default();

    let cpu = CpuSettings {
        governor: section_value(&ini, profile_dir, "cpu", "governor"),
        energy_performance_preference: section_value(
            &ini,
            profile_dir,
            "cpu",
            "energy_performance_preference",
        ),
    };

    let vm = VmSettings {
        transparent_hugepages: section_value(&ini, profile_dir, "vm", "transparent_hugepages")
            .or_else(|| section_value(&ini, profile_dir, "vm", "transparent_hugepage")),
        transparent_hugepage_defrag: section_value(
            &ini,
            profile_dir,
            "vm",
            "transparent_hugepage.defrag",
        ),
        dirty_bytes: section_value(&ini, profile_dir, "vm", "dirty_bytes"),
        dirty_ratio: section_value(&ini, profile_dir, "vm", "dirty_ratio"),
        dirty_background_bytes: section_value(&ini, profile_dir, "vm", "dirty_background_bytes"),
        dirty_background_ratio: section_value(&ini, profile_dir, "vm", "dirty_background_ratio"),
    };

    let disk = DiskSettings {
        devices: section_value(&ini, profile_dir, "disk", "devices"),
        elevator: section_value(&ini, profile_dir, "disk", "elevator"),
        readahead: section_value(&ini, profile_dir, "disk", "readahead"),
    };

    let acpi = AcpiSettings {
        platform_profile: section_value(&ini, profile_dir, "acpi", "platform_profile"),
    };

    let network = NetworkSettings {
        tcp_congestion_control: section_value(
            &ini,
            profile_dir,
            "network",
            "tcp_congestion_control",
        ),
        tcp_window_scaling: section_value(&ini, profile_dir, "network", "tcp_window_scaling"),
        tcp_timestamps: section_value(&ini, profile_dir, "network", "tcp_timestamps"),
        tcp_sack: section_value(&ini, profile_dir, "network", "tcp_sack"),
        tcp_fastopen: section_value(&ini, profile_dir, "network", "tcp_fastopen"),
    };

    let mut sysctl = HashMap::new();
    if let Some(section) = ini.get_map_ref().get("sysctl") {
        for (key, value) in section {
            if let Some(value) = value {
                sysctl.insert(key.clone(), expand_profile_dir(value.trim(), profile_dir));
            }
        }
    }

    Ok(Profile {
        name: name.to_string(),
        summary,
        description,
        cpu,
        vm,
        disk,
        acpi,
        network,
        sysctl,
        gpu: section_options(&ini, profile_dir, "gpu"),
        storage: section_options(&ini, profile_dir, "storage"),
        thermal: section_options(&ini, profile_dir, "thermal"),
        battery: section_options(&ini, profile_dir, "battery"),
        hermes: section_options(&ini, profile_dir, "hermes"),
    })
}

fn read_ini(path: &Path) -> Result<Ini> {
    let mut ini = Ini::new();
    ini.load(path.to_str().unwrap_or_default())
        .map_err(|error| anyhow::anyhow!("Failed to parse {}: {error}", path.display()))?;
    Ok(ini)
}

fn section_value(ini: &Ini, profile_dir: &Path, section: &str, key: &str) -> Option<String> {
    ini.get(section, key)
        .map(|value| expand_profile_dir(value.trim(), profile_dir))
        .filter(|value| !value.is_empty())
}

fn section_options(ini: &Ini, profile_dir: &Path, section: &str) -> PluginOptions {
    let mut options = ini
        .get_map_ref()
        .get(section)
        .into_iter()
        .flat_map(|entries| entries.iter())
        .filter_map(|(key, value)| {
            value
                .as_ref()
                .map(|value| (key.clone(), expand_profile_dir(value.trim(), profile_dir)))
        })
        .filter(|(_, value)| !value.is_empty())
        .collect::<Vec<_>>();
    options.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    options
}

fn expand_profile_dir(value: &str, profile_dir: &Path) -> String {
    const MARKER: &str = "${i:PROFILE_DIR}";
    const ESCAPED_MARKER: &str = "\\${i:PROFILE_DIR}";
    const PLACEHOLDER: &str = "\u{1f}TUNED_PROFILE_DIR\u{1f}";

    value
        .replace(ESCAPED_MARKER, PLACEHOLDER)
        .replace(MARKER, &profile_dir.to_string_lossy())
        .replace(PLACEHOLDER, ESCAPED_MARKER)
}

fn merge_optional<T>(current: &mut Option<T>, newer: Option<T>) {
    if newer.is_some() {
        *current = newer;
    }
}

fn merge_plugin_options(current: &mut PluginOptions, newer: PluginOptions) {
    for (key, value) in newer {
        if let Some((_, current_value)) = current.iter_mut().find(|(name, _)| name == &key) {
            *current_value = value;
        } else {
            current.push((key, value));
        }
    }
}

pub fn read_active_profile() -> Result<Option<String>> {
    let path = config::resolve_path(config::ACTIVE_PROFILE_FILE);
    if !path.is_file() {
        return Ok(None);
    }

    let content =
        fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    let name = content.trim();
    if name.is_empty() {
        return Ok(None);
    }
    let normalized = engine::normalize_profile_selection(name)
        .with_context(|| format!("Invalid active profile in {}", path.display()))?;

    Ok(Some(normalized))
}

pub fn save_active_profile(name: &str) -> Result<()> {
    let normalized = engine::normalize_profile_selection(name)
        .with_context(|| format!("Invalid profile name '{name}'"))?;

    let path = config::resolve_path(config::ACTIVE_PROFILE_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    fs::write(&path, format!("{normalized}\n"))
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

pub fn save_profile_mode(manual: bool) -> Result<()> {
    let path = config::resolve_path(config::PROFILE_MODE_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let mode = if manual { "manual" } else { "auto" };
    fs::write(&path, format!("{mode}\n"))
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn option_value<'a>(options: &'a PluginOptions, key: &str) -> Option<&'a str> {
        options
            .iter()
            .find_map(|(name, value)| (name == key).then_some(value.as_str()))
    }

    fn write_profile(root: &Path, name: &str, body: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(PROFILE_FILE), body).unwrap();
    }

    #[test]
    fn parses_extended_profile_sections() {
        let dir = TempDir::new().unwrap();
        let profile_dir = dir.path().join("performance");
        fs::create_dir_all(&profile_dir).unwrap();
        let mut file = fs::File::create(profile_dir.join(PROFILE_FILE)).unwrap();
        writeln!(
            file,
            "[main]\nsummary=Performance\n\n[cpu]\ngovernor=performance\n\n[vm]\ndirty_bytes=40%\n\n[disk]\nreadahead=>4096\n\n[acpi]\nplatform_profile=performance|balanced\n\n[sysctl]\nvm.swappiness=10\nnet.core.somaxconn=>2048\n\n[gpu]\nnvidia_power_limit=250\namd_power_profile=high\n\n[storage]\nnvme_apst=0\nio_scheduler=mq-deadline\n\n[thermal]\ncpu_temp_limit=85\nfan_control=auto\n\n[battery]\ncharge_start_threshold=20\ncharge_stop_threshold=80\n\n[hermes]\ngsp_power_mode=performance\ncmd_ring_size=4096\n"
        )
        .unwrap();

        let profile = load_profile(&profile_dir.join(PROFILE_FILE), "performance").unwrap();
        assert_eq!(profile.cpu.governor.as_deref(), Some("performance"));
        assert_eq!(profile.vm.dirty_bytes.as_deref(), Some("40%"));
        assert_eq!(profile.disk.readahead.as_deref(), Some(">4096"));
        assert_eq!(
            profile.acpi.platform_profile.as_deref(),
            Some("performance|balanced")
        );
        assert_eq!(
            profile.sysctl.get("net.core.somaxconn"),
            Some(&">2048".to_string())
        );
        assert_eq!(
            option_value(&profile.gpu, "nvidia_power_limit"),
            Some("250")
        );
        assert_eq!(
            option_value(&profile.gpu, "amd_power_profile"),
            Some("high")
        );
        assert_eq!(option_value(&profile.storage, "nvme_apst"), Some("0"));
        assert_eq!(
            option_value(&profile.storage, "io_scheduler"),
            Some("mq-deadline")
        );
        assert_eq!(option_value(&profile.thermal, "cpu_temp_limit"), Some("85"));
        assert_eq!(option_value(&profile.thermal, "fan_control"), Some("auto"));
        assert_eq!(
            option_value(&profile.battery, "charge_start_threshold"),
            Some("20")
        );
        assert_eq!(
            option_value(&profile.battery, "charge_stop_threshold"),
            Some("80")
        );
        assert_eq!(
            option_value(&profile.hermes, "gsp_power_mode"),
            Some("performance")
        );
        assert_eq!(option_value(&profile.hermes, "cmd_ring_size"), Some("4096"));
    }

    #[test]
    fn later_profile_dir_overrides_name() {
        let root = TempDir::new().unwrap();
        let system = root.path().join("usr/lib/tuned/profiles");
        let user = root.path().join("etc/tuned/profiles");
        write_profile(&system, "balanced", "[main]\nsummary=system\n");
        write_profile(&user, "balanced", "[main]\nsummary=custom\n");

        let catalog = ProfileCatalog::load_from_dirs(&[system, user]).unwrap();
        assert_eq!(catalog.get("balanced").unwrap().summary, "custom");
    }

    #[test]
    fn included_profiles_merge_before_the_child() {
        let root = TempDir::new().unwrap();
        let profiles = root.path().join("profiles");
        write_profile(
            &profiles,
            "base",
            "[main]\nsummary=Base\ndescription=Inherited description\n\n[cpu]\ngovernor=powersave\n\n[sysctl]\nvm.swappiness=20\n\n[gpu]\nnvidia_power_limit=200\n",
        );
        write_profile(
            &profiles,
            "child",
            "[main]\ninclude=base\nsummary=Child\n\n[cpu]\nenergy_performance_preference=performance\n\n[sysctl]\nvm.swappiness=10\n\n[gpu]\nnvidia_power_limit=250\n",
        );

        let catalog = ProfileCatalog::load_from_dirs(&[profiles]).unwrap();
        let child = catalog.get("child").unwrap();
        assert_eq!(child.summary, "Child");
        assert_eq!(child.description, "Inherited description");
        assert_eq!(child.cpu.governor.as_deref(), Some("powersave"));
        assert_eq!(
            child.cpu.energy_performance_preference.as_deref(),
            Some("performance")
        );
        assert_eq!(child.sysctl.get("vm.swappiness"), Some(&"10".to_string()));
        assert_eq!(option_value(&child.gpu, "nvidia_power_limit"), Some("250"));
        assert_eq!(
            catalog.profile_info("child"),
            (
                true,
                "child".to_string(),
                "Child".to_string(),
                "Inherited description".to_string()
            )
        );
    }

    #[test]
    fn stacked_profiles_merge_in_requested_order() {
        let root = TempDir::new().unwrap();
        let profiles = root.path().join("profiles");
        write_profile(
            &profiles,
            "first",
            "[main]\nsummary=First\n\n[cpu]\ngovernor=powersave\n\n[sysctl]\nvm.swappiness=20\n",
        );
        write_profile(
            &profiles,
            "second",
            "[main]\nsummary=Second\n\n[cpu]\nenergy_performance_preference=performance\n\n[sysctl]\nvm.swappiness=5\n",
        );

        let catalog = ProfileCatalog::load_from_dirs(&[profiles]).unwrap();
        let stacked = catalog.resolve("first   second").unwrap();
        assert_eq!(stacked.name, "first second");
        assert_eq!(stacked.summary, "Second");
        assert_eq!(stacked.cpu.governor.as_deref(), Some("powersave"));
        assert_eq!(
            stacked.cpu.energy_performance_preference.as_deref(),
            Some("performance")
        );
        assert_eq!(stacked.sysctl.get("vm.swappiness"), Some(&"5".to_string()));
    }

    #[test]
    fn conditional_missing_include_is_ignored() {
        let root = TempDir::new().unwrap();
        let profiles = root.path().join("profiles");
        write_profile(
            &profiles,
            "child",
            "[main]\ninclude=-not-installed\nsummary=Child\n",
        );

        let catalog = ProfileCatalog::load_from_dirs(&[profiles]).unwrap();
        assert_eq!(catalog.get("child").unwrap().summary, "Child");
    }

    #[test]
    fn recursive_includes_are_loaded_only_once() {
        let root = TempDir::new().unwrap();
        let profiles = root.path().join("profiles");
        write_profile(
            &profiles,
            "first",
            "[main]\ninclude=second\n\n[cpu]\ngovernor=performance\n",
        );
        write_profile(
            &profiles,
            "second",
            "[main]\ninclude=first\n\n[cpu]\nenergy_performance_preference=balance_performance\n",
        );

        let catalog = ProfileCatalog::load_from_dirs(&[profiles]).unwrap();
        let first = catalog.get("first").unwrap();
        assert_eq!(first.cpu.governor.as_deref(), Some("performance"));
        assert_eq!(
            first.cpu.energy_performance_preference.as_deref(),
            Some("balance_performance")
        );
    }

    #[test]
    fn expands_profile_directory_marker() {
        let root = TempDir::new().unwrap();
        let profiles = root.path().join("profiles");
        write_profile(
            &profiles,
            "scripted",
            "[main]\nsummary=Scripted\n\n[hermes]\nfirmware_validation=${i:PROFILE_DIR}/allowlist\ndebug_level=\\${i:PROFILE_DIR}\n",
        );

        let catalog = ProfileCatalog::load_from_dirs(&[profiles]).unwrap();
        let scripted = catalog.get("scripted").unwrap();
        let profile_dir = root.path().join("profiles/scripted");
        assert_eq!(
            option_value(&scripted.hermes, "firmware_validation"),
            Some(profile_dir.join("allowlist").to_string_lossy().as_ref())
        );
        assert_eq!(
            option_value(&scripted.hermes, "debug_level"),
            Some("\\${i:PROFILE_DIR}")
        );
    }
}
