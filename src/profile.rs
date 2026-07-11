use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
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

#[derive(Debug, Clone)]
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
}

#[derive(Debug, Clone)]
pub struct ProfileCatalog {
    profiles: HashMap<String, Profile>,
}

impl ProfileCatalog {
    pub fn load_from_dirs(dirs: &[PathBuf]) -> Result<Self> {
        let mut profiles = HashMap::new();

        for dir in dirs {
            if !dir.is_dir() {
                warn!("Profile directory does not exist: {}", dir.display());
                continue;
            }
            scan_profile_dir(dir, &mut profiles)?;
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

    pub fn profile_info(&self, name: &str) -> (bool, String, String, String) {
        match self.profiles.get(name) {
            Some(profile) => (
                true,
                profile.summary.clone(),
                profile.description.clone(),
                String::new(),
            ),
            None => (
                false,
                String::new(),
                String::new(),
                format!("Profile '{name}' not found"),
            ),
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

fn scan_profile_dir(dir: &Path, profiles: &mut HashMap<String, Profile>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        if !engine::validate_profile_name(&name) {
            continue;
        }

        let conf_path = entry.path().join(PROFILE_FILE);
        if !conf_path.is_file() {
            continue;
        }

        match load_profile(&conf_path, &name) {
            Ok(profile) => {
                profiles.insert(name, profile);
            }
            Err(error) => warn!("Skipping profile '{name}': {error}"),
        }
    }

    Ok(())
}

pub fn load_profile(path: &Path, name: &str) -> Result<Profile> {
    let mut ini = Ini::new();
    ini.load(path.to_str().unwrap_or_default())
        .map_err(|error| anyhow::anyhow!("Failed to parse {}: {error}", path.display()))?;

    let summary = ini
        .get("main", "summary")
        .unwrap_or_default()
        .trim()
        .to_string();
    let description = ini
        .get("main", "description")
        .unwrap_or_default()
        .trim()
        .to_string();

    let cpu = CpuSettings {
        governor: section_value(&ini, "cpu", "governor"),
        energy_performance_preference: section_value(
            &ini,
            "cpu",
            "energy_performance_preference",
        ),
    };

    let vm = VmSettings {
        transparent_hugepages: section_value(&ini, "vm", "transparent_hugepages")
            .or_else(|| section_value(&ini, "vm", "transparent_hugepage")),
        transparent_hugepage_defrag: section_value(&ini, "vm", "transparent_hugepage.defrag"),
        dirty_bytes: section_value(&ini, "vm", "dirty_bytes"),
        dirty_ratio: section_value(&ini, "vm", "dirty_ratio"),
        dirty_background_bytes: section_value(&ini, "vm", "dirty_background_bytes"),
        dirty_background_ratio: section_value(&ini, "vm", "dirty_background_ratio"),
    };

    let disk = DiskSettings {
        devices: section_value(&ini, "disk", "devices"),
        elevator: section_value(&ini, "disk", "elevator"),
        readahead: section_value(&ini, "disk", "readahead"),
    };

    let acpi = AcpiSettings {
        platform_profile: section_value(&ini, "acpi", "platform_profile"),
    };

    let network = NetworkSettings {
        tcp_congestion_control: section_value(&ini, "network", "tcp_congestion_control"),
        tcp_window_scaling: section_value(&ini, "network", "tcp_window_scaling"),
        tcp_timestamps: section_value(&ini, "network", "tcp_timestamps"),
        tcp_sack: section_value(&ini, "network", "tcp_sack"),
        tcp_fastopen: section_value(&ini, "network", "tcp_fastopen"),
    };

    let mut sysctl = HashMap::new();
    if let Some(section) = ini.get_map_ref().get("sysctl") {
        for (key, value) in section {
            if let Some(value) = value {
                sysctl.insert(key.clone(), value.trim().to_string());
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
    })
}

fn section_value(ini: &Ini, section: &str, key: &str) -> Option<String> {
    ini.get(section, key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn read_active_profile() -> Result<Option<String>> {
    let path = config::resolve_path(config::ACTIVE_PROFILE_FILE);
    if !path.is_file() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let name = content.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if !engine::validate_profile_name(name) {
        bail!("Invalid active profile in {}", path.display());
    }

    Ok(Some(name.to_string()))
}

pub fn save_active_profile(name: &str) -> Result<()> {
    if !engine::validate_profile_name(name) {
        bail!("Invalid profile name '{name}'");
    }

    let path = config::resolve_path(config::ACTIVE_PROFILE_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    fs::write(&path, format!("{name}\n"))
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

    #[test]
    fn parses_extended_profile_sections() {
        let dir = TempDir::new().unwrap();
        let profile_dir = dir.path().join("performance");
        fs::create_dir_all(&profile_dir).unwrap();
        let mut file = fs::File::create(profile_dir.join(PROFILE_FILE)).unwrap();
        writeln!(
            file,
            "[main]\nsummary=Performance\n\n[cpu]\ngovernor=performance\n\n[vm]\ndirty_bytes=40%\n\n[disk]\nreadahead=>4096\n\n[acpi]\nplatform_profile=performance|balanced\n\n[sysctl]\nvm.swappiness=10\nnet.core.somaxconn=>2048\n"
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
    }

    #[test]
    fn later_profile_dir_overrides_name() {
        let root = TempDir::new().unwrap();
        let system = root.path().join("usr/lib/tuned/profiles");
        let user = root.path().join("etc/tuned/profiles");
        for (base, summary) in [(&system, "system"), (&user, "custom")] {
            let profile_dir = base.join("balanced");
            fs::create_dir_all(&profile_dir).unwrap();
            fs::write(
                profile_dir.join(PROFILE_FILE),
                format!("[main]\nsummary={summary}\n"),
            )
            .unwrap();
        }

        let catalog =
            ProfileCatalog::load_from_dirs(&[system.clone(), user.clone()]).unwrap();
        assert_eq!(catalog.get("balanced").unwrap().summary, "custom");
    }
}
