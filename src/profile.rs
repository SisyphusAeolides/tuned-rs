use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use configparser::ini::Ini;
use tracing::{info, warn};

use crate::config::{self, PROFILE_FILE};
use crate::engine;
use crate::profile_units::{
    merge_options, merge_units, merge_variables, option_value, OrderedOptions, ProfileUnit,
};

#[derive(Debug, Clone, Default)]
pub struct CpuSettings {
    pub governor: Option<String>,
    pub energy_perf_bias: Option<String>,
    pub energy_performance_preference: Option<String>,
    pub min_perf_pct: Option<String>,
    pub max_perf_pct: Option<String>,
    pub boost: Option<String>,
    pub force_latency: Option<String>,
    pub pm_qos_resume_latency_us: Option<String>,
    pub sampling_down_factor: Option<String>,
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
    pub tcp_rmem: Option<String>,
    pub tcp_wmem: Option<String>,
    pub tcp_max_syn_backlog: Option<String>,
    pub tcp_tw_reuse: Option<String>,
    pub tcp_fin_timeout: Option<String>,
    pub core_rmem_max: Option<String>,
    pub core_wmem_max: Option<String>,
    pub core_netdev_max_backlog: Option<String>,
    pub core_somaxconn: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AcpiSettings {
    pub platform_profile: Option<String>,
}

pub type PluginOptions = OrderedOptions;

#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub name: String,
    pub summary: String,
    pub description: String,
    pub main_options: PluginOptions,
    pub variables: PluginOptions,
    pub units: Vec<ProfileUnit>,
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
    variable_policy: Option<(bool, bool)>,
}

impl Profile {
    fn merge_from(&mut self, newer: Profile) {
        if !newer.summary.is_empty() {
            self.summary = newer.summary;
        }
        if !newer.description.is_empty() {
            self.description = newer.description;
        }

        merge_options(&mut self.main_options, newer.main_options);
        if !newer.variables.is_empty() || newer.variable_policy.is_some() {
            let (replace, prepend) = newer.variable_policy.unwrap_or((false, false));
            merge_variables(&mut self.variables, newer.variables, replace, prepend);
            if newer.variable_policy.is_some() {
                self.variable_policy = newer.variable_policy;
            }
        }
        merge_units(&mut self.units, newer.units);
        self.rebuild_legacy_projection();
    }

    pub fn units_of_type<'a>(
        &'a self,
        plugin_type: &'a str,
    ) -> impl Iterator<Item = &'a ProfileUnit> + 'a {
        self.units
            .iter()
            .filter(move |unit| unit.plugin_type == plugin_type)
    }

    fn rebuild_legacy_projection(&mut self) {
        let mut cpu = CpuSettings::default();
        let mut vm = VmSettings::default();
        let mut disk = DiskSettings::default();
        let mut acpi = AcpiSettings::default();
        let mut network = NetworkSettings::default();
        let mut sysctl = HashMap::new();
        let mut gpu = PluginOptions::new();
        let mut storage = PluginOptions::new();
        let mut thermal = PluginOptions::new();
        let mut battery = PluginOptions::new();
        let mut hermes = PluginOptions::new();

        for unit in &self.units {
            if !unit.enabled || !projection_is_safe(unit) {
                continue;
            }
            match unit.plugin_type.as_str() {
                "cpu" => {
                    set_from_unit(&mut cpu.governor, unit, "governor");
                    set_from_unit(&mut cpu.energy_perf_bias, unit, "energy_perf_bias");
                    set_from_unit(
                        &mut cpu.energy_performance_preference,
                        unit,
                        "energy_performance_preference",
                    );
                    set_from_unit(&mut cpu.min_perf_pct, unit, "min_perf_pct");
                    set_from_unit(&mut cpu.max_perf_pct, unit, "max_perf_pct");
                    set_from_unit(&mut cpu.boost, unit, "boost");
                    set_from_unit(&mut cpu.force_latency, unit, "force_latency");
                    set_from_unit(
                        &mut cpu.pm_qos_resume_latency_us,
                        unit,
                        "pm_qos_resume_latency_us",
                    );
                    set_from_unit(&mut cpu.sampling_down_factor, unit, "sampling_down_factor");
                }
                "vm" => {
                    set_from_unit_aliases(
                        &mut vm.transparent_hugepages,
                        unit,
                        &["transparent_hugepages", "transparent_hugepage"],
                    );
                    set_from_unit(
                        &mut vm.transparent_hugepage_defrag,
                        unit,
                        "transparent_hugepage.defrag",
                    );
                    set_from_unit(&mut vm.dirty_bytes, unit, "dirty_bytes");
                    set_from_unit(&mut vm.dirty_ratio, unit, "dirty_ratio");
                    set_from_unit(
                        &mut vm.dirty_background_bytes,
                        unit,
                        "dirty_background_bytes",
                    );
                    set_from_unit(
                        &mut vm.dirty_background_ratio,
                        unit,
                        "dirty_background_ratio",
                    );
                }
                "disk" => {
                    if unit.devices != "*" {
                        disk.devices = Some(unit.devices.clone());
                    }
                    set_from_unit(&mut disk.elevator, unit, "elevator");
                    set_from_unit(&mut disk.readahead, unit, "readahead");
                }
                "acpi" => {
                    set_from_unit(&mut acpi.platform_profile, unit, "platform_profile");
                }
                "network" | "net" => {
                    set_from_unit(
                        &mut network.tcp_congestion_control,
                        unit,
                        "tcp_congestion_control",
                    );
                    set_from_unit(&mut network.tcp_window_scaling, unit, "tcp_window_scaling");
                    set_from_unit(&mut network.tcp_timestamps, unit, "tcp_timestamps");
                    set_from_unit(&mut network.tcp_sack, unit, "tcp_sack");
                    set_from_unit(&mut network.tcp_fastopen, unit, "tcp_fastopen");
                    set_from_unit(&mut network.tcp_rmem, unit, "tcp_rmem");
                    set_from_unit(&mut network.tcp_wmem, unit, "tcp_wmem");
                    set_from_unit(
                        &mut network.tcp_max_syn_backlog,
                        unit,
                        "tcp_max_syn_backlog",
                    );
                    set_from_unit(&mut network.tcp_tw_reuse, unit, "tcp_tw_reuse");
                    set_from_unit(&mut network.tcp_fin_timeout, unit, "tcp_fin_timeout");
                    set_from_unit(&mut network.core_rmem_max, unit, "core_rmem_max");
                    set_from_unit(&mut network.core_wmem_max, unit, "core_wmem_max");
                    set_from_unit(
                        &mut network.core_netdev_max_backlog,
                        unit,
                        "core_netdev_max_backlog",
                    );
                    set_from_unit(&mut network.core_somaxconn, unit, "core_somaxconn");
                }
                "sysctl" => {
                    for (key, value) in &unit.options {
                        sysctl.insert(key.clone(), value.clone());
                    }
                }
                "gpu" => merge_options(&mut gpu, unit.options.clone()),
                "storage" => merge_options(&mut storage, unit.options.clone()),
                "thermal" => merge_options(&mut thermal, unit.options.clone()),
                "battery" => merge_options(&mut battery, unit.options.clone()),
                "hermes" => merge_options(&mut hermes, unit.options.clone()),
                _ => {}
            }
        }

        self.cpu = cpu;
        self.vm = vm;
        self.disk = disk;
        self.acpi = acpi;
        self.network = network;
        self.sysctl = sysctl;
        self.gpu = gpu;
        self.storage = storage;
        self.thermal = thermal;
        self.battery = battery;
        self.hermes = hermes;
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
                    profile.rebuild_legacy_projection();
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
        profile.rebuild_legacy_projection();
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
    let include = section_value(&ini, profile_dir, "main", "include").unwrap_or_default();

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
    let main_options = section_options(&ini, profile_dir, "main");
    let summary = option_value(&main_options, "summary")
        .unwrap_or_default()
        .to_string();
    let description = option_value(&main_options, "description")
        .unwrap_or_default()
        .to_string();

    let mut variables = PluginOptions::new();
    let mut variable_policy = None;
    let mut units = Vec::new();
    for section in ordered_section_names(path, &ini)? {
        if section.eq_ignore_ascii_case("main") {
            continue;
        }
        let unit =
            ProfileUnit::from_options(&section, section_options(&ini, profile_dir, &section))?;
        if unit.plugin_type == "variables" {
            variable_policy = Some((unit.replace, unit.prepend));
            merge_variables(&mut variables, unit.options, unit.replace, unit.prepend);
        } else {
            units.push(unit);
        }
    }

    let mut profile = Profile {
        name: name.to_string(),
        summary,
        description,
        main_options,
        variables,
        units,
        variable_policy,
        ..Profile::default()
    };
    profile.rebuild_legacy_projection();
    Ok(profile)
}

fn read_ini(path: &Path) -> Result<Ini> {
    let mut ini = Ini::new();
    ini.load(path.to_str().unwrap_or_default())
        .map_err(|error| anyhow::anyhow!("Failed to parse {}: {error}", path.display()))?;
    Ok(ini)
}

fn ordered_section_names(path: &Path, ini: &Ini) -> Result<Vec<String>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mut sections = Vec::new();
    let mut seen = HashSet::new();

    for line in content.lines() {
        let line = line.trim();
        if !line.starts_with('[') {
            continue;
        }
        let Some(end) = line.find(']') else {
            continue;
        };
        let section = line[1..end].trim();
        if section.is_empty() {
            continue;
        }
        let key = section.to_ascii_lowercase();
        if seen.insert(key) {
            sections.push(section.to_string());
        }
    }

    let mut remaining = ini
        .get_map_ref()
        .keys()
        .filter(|section| !seen.contains(&section.to_ascii_lowercase()))
        .cloned()
        .collect::<Vec<_>>();
    remaining.sort_unstable();
    sections.extend(remaining);
    Ok(sections)
}

fn section_value(ini: &Ini, profile_dir: &Path, section: &str, key: &str) -> Option<String> {
    section_entries(ini, section)
        .and_then(|entries| {
            entries
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
                .and_then(|(_, value)| value.as_ref())
        })
        .map(|value| expand_profile_dir(value.trim(), profile_dir))
        .filter(|value| !value.is_empty())
}

fn section_options(ini: &Ini, profile_dir: &Path, section: &str) -> PluginOptions {
    let mut options = section_entries(ini, section)
        .into_iter()
        .flat_map(|entries| entries.iter())
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_ref()
                    .map(|value| expand_profile_dir(value.trim(), profile_dir))
                    .unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    options.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    options
}

fn section_entries<'a>(ini: &'a Ini, section: &str) -> Option<&'a HashMap<String, Option<String>>> {
    ini.get_map_ref().get(section).or_else(|| {
        ini.get_map_ref()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(section))
            .map(|(_, entries)| entries)
    })
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

fn projection_is_safe(unit: &ProfileUnit) -> bool {
    unit.devices_udev_regex.is_none()
        && unit.cpuinfo_regex.is_none()
        && unit.uname_regex.is_none()
        && (unit.devices == "*" || unit.plugin_type == "disk")
}

fn set_from_unit(target: &mut Option<String>, unit: &ProfileUnit, option: &str) {
    if let Some(value) = unit.option(option) {
        *target = Some(value.to_string());
    }
}

fn set_from_unit_aliases(target: &mut Option<String>, unit: &ProfileUnit, options: &[&str]) {
    for option in options {
        if let Some(value) = unit.option(option) {
            *target = Some(value.to_string());
            return;
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
        crate::profile_units::option_value(options, key)
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
        assert_eq!(profile.sysctl.get("vm.swappiness"), Some(&"10".to_string()));
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
    fn preserves_named_units_and_skips_conditional_projection() {
        let root = TempDir::new().unwrap();
        let profiles = root.path().join("profiles");
        write_profile(
            &profiles,
            "server",
            "[main]\nsummary=Server\n\n[variables]\nthunderx=CPU part.*516\n\n[vm]\ndirty_ratio=20\n\n[vm.thunderx]\ntype=vm\nuname_regex=aarch64\ncpuinfo_regex=${thunderx}\ntransparent_hugepages=never\n\n[sysctl.thunderx]\ntype=sysctl\nuname_regex=aarch64\nkernel.numa_balancing=0\n",
        );

        let catalog = ProfileCatalog::load_from_dirs(&[profiles]).unwrap();
        let profile = catalog.get("server").unwrap();
        assert_eq!(profile.units.len(), 3);
        let thunderx = profile
            .units
            .iter()
            .find(|unit| unit.name == "vm.thunderx")
            .unwrap();
        assert_eq!(thunderx.plugin_type, "vm");
        assert_eq!(thunderx.uname_regex.as_deref(), Some("aarch64"));
        assert_eq!(thunderx.cpuinfo_regex.as_deref(), Some("${thunderx}"));
        assert_eq!(thunderx.option("transparent_hugepages"), Some("never"));
        assert_eq!(profile.vm.dirty_ratio.as_deref(), Some("20"));
        assert_eq!(profile.vm.transparent_hugepages, None);
        assert!(!profile.sysctl.contains_key("kernel.numa_balancing"));
        assert_eq!(
            option_value(&profile.variables, "thunderx"),
            Some("CPU part.*516")
        );
    }

    #[test]
    fn unit_drop_and_replace_match_upstream_merge_rules() {
        let root = TempDir::new().unwrap();
        let profiles = root.path().join("profiles");
        write_profile(
            &profiles,
            "base",
            "[main]\nsummary=Base\n\n[cpu]\ngovernor=powersave\nboost=0\n\n[sysctl]\nvm.swappiness=60\n",
        );
        write_profile(
            &profiles,
            "child",
            "[main]\ninclude=base\n\n[cpu]\ndrop=boost\ngovernor=performance\n\n[sysctl]\nreplace=true\nkernel.nmi_watchdog=0\n",
        );

        let catalog = ProfileCatalog::load_from_dirs(&[profiles]).unwrap();
        let child = catalog.get("child").unwrap();
        let cpu = child.units.iter().find(|unit| unit.name == "cpu").unwrap();
        assert_eq!(cpu.option("governor"), Some("performance"));
        assert_eq!(cpu.option("boost"), None);
        assert_eq!(child.cpu.governor.as_deref(), Some("performance"));
        assert_eq!(child.cpu.boost, None);
        assert!(!child.sysctl.contains_key("vm.swappiness"));
        assert_eq!(
            child.sysctl.get("kernel.nmi_watchdog"),
            Some(&"0".to_string())
        );
    }

    #[test]
    fn net_is_an_upstream_alias_for_network() {
        let root = TempDir::new().unwrap();
        let profiles = root.path().join("profiles");
        write_profile(
            &profiles,
            "networked",
            "[main]\nsummary=Networked\n\n[net]\ntcp_fastopen=3\ncore_somaxconn=4096\n",
        );

        let catalog = ProfileCatalog::load_from_dirs(&[profiles]).unwrap();
        let profile = catalog.get("networked").unwrap();
        assert_eq!(profile.network.tcp_fastopen.as_deref(), Some("3"));
        assert_eq!(profile.network.core_somaxconn.as_deref(), Some("4096"));
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
