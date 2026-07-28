use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RollbackFile {
    #[serde(default)]
    entries: HashMap<String, String>,
    #[serde(default)]
    order: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManagedFileSnapshot {
    existed: bool,
    contents: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
struct RollbackState {
    entries: HashMap<String, String>,
    order: Vec<String>,
}

pub struct Rollback {
    path: PathBuf,
    state: Mutex<RollbackState>,
    managed_files: Vec<PathBuf>,
    cleanup_runtime_resources: bool,
}

impl Rollback {
    pub fn load() -> Result<Self> {
        Self::load_from_path(config::resolve_path(config::ROLLBACK_FILE))
    }

    pub fn load_instance(instance_name: &str) -> Result<Self> {
        let mut rollback = Self::load_from_path(config::resolve_path(&format!(
            "/var/lib/tuned-rs/instances/{instance_name}.json"
        )))?;
        rollback.cleanup_runtime_resources = false;
        Ok(rollback)
    }

    fn load_from_path(path: PathBuf) -> Result<Self> {
        Self::load_from_path_with_managed_files(path, default_managed_files())
    }

    fn load_from_path_with_managed_files(
        path: PathBuf,
        managed_files: Vec<PathBuf>,
    ) -> Result<Self> {
        let persisted = if path.is_file() {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            serde_json::from_str::<RollbackFile>(&content)
                .with_context(|| format!("Failed to parse {}", path.display()))?
        } else {
            RollbackFile::default()
        };

        let mut state = RollbackState {
            entries: persisted.entries,
            order: persisted.order,
        };
        normalize_order(&mut state);

        if !state.entries.is_empty() {
            info!("Loaded {} rollback entries from disk", state.entries.len());
        }

        Ok(Self {
            path,
            state: Mutex::new(state),
            managed_files,
            cleanup_runtime_resources: true,
        })
    }

    pub fn record_original(&self, key: &str, original: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.entries.contains_key(key) {
            return Ok(());
        }

        let mut next = state.clone();
        next.entries.insert(key.to_string(), original.to_string());
        next.order.push(key.to_string());
        self.persist_state(&next)?;
        *state = next;
        Ok(())
    }

    pub fn record_managed_file(&self, path: &Path) -> Result<()> {
        validate_managed_file(path, &self.managed_files)?;
        self.record_file_snapshot("file", path)
    }

    pub fn record_boot_file(&self, path: &Path) -> Result<()> {
        validate_boot_file(path)?;
        self.record_file_snapshot("bootfile", path)
    }

    pub fn record_grub_file(&self, path: &Path) -> Result<()> {
        validate_grub_file(path)?;
        self.record_file_snapshot("grubfile", path)
    }

    pub fn record_systemd_dropin(&self, path: &Path) -> Result<()> {
        validate_systemd_dropin(path)?;
        self.record_file_snapshot("systemdfile", path)
    }

    fn record_file_snapshot(&self, kind: &str, path: &Path) -> Result<()> {
        let snapshot = match fs::read(path) {
            Ok(contents) => ManagedFileSnapshot {
                existed: true,
                contents,
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => ManagedFileSnapshot {
                existed: false,
                contents: Vec::new(),
            },
            Err(error) => {
                return Err(error).with_context(|| format!("Failed to snapshot {}", path.display()))
            }
        };
        let encoded = serde_json::to_string(&snapshot)?;
        self.record_original(&rollback_key(kind, &path.to_string_lossy()), &encoded)
    }

    pub fn restore_all(&self) -> Result<()> {
        if self.cleanup_runtime_resources {
            crate::tuning::cleanup_runtime_resources();
        }
        let managed_files = self.managed_files.clone();
        self.restore_with(move |key, original| restore_entry(key, original, &managed_files))
    }

    fn restore_with<F>(&self, mut restore: F) -> Result<()>
    where
        F: FnMut(&str, &str) -> Result<()>,
    {
        let mut state = self.state.lock().unwrap();
        if state.entries.is_empty() {
            return Ok(());
        }

        normalize_order(&mut state);
        info!("Restoring {} tuned value(s)", state.entries.len());

        let snapshot = state.clone();
        let mut remaining = snapshot.clone();
        let mut failures = Vec::new();

        for key in snapshot.order.iter().rev() {
            let Some(original) = snapshot.entries.get(key) else {
                continue;
            };
            match restore(key, original) {
                Ok(()) => {
                    remaining.entries.remove(key);
                    remaining.order.retain(|entry| entry != key);
                }
                Err(error) => {
                    warn!("Failed to restore '{key}': {error}");
                    failures.push(format!("{key}: {error}"));
                }
            }
        }

        self.persist_state(&remaining)?;
        *state = remaining;

        if failures.is_empty() {
            Ok(())
        } else {
            bail!(
                "Failed to restore {} tuned value(s): {}",
                failures.len(),
                failures.join("; ")
            )
        }
    }

    pub fn clear(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let empty = RollbackState::default();
        self.persist_state(&empty)?;
        *state = empty;
        Ok(())
    }

    fn persist_state(&self, state: &RollbackState) -> Result<()> {
        if state.entries.is_empty() {
            if self.path.is_file() {
                fs::remove_file(&self.path)
                    .with_context(|| format!("Failed to remove {}", self.path.display()))?;
            }
            return Ok(());
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }

        let snapshot = RollbackFile {
            entries: state.entries.clone(),
            order: state.order.clone(),
        };
        let content = serde_json::to_string_pretty(&snapshot)?;
        let temporary = self.path.with_extension("tmp");
        fs::write(&temporary, content)
            .with_context(|| format!("Failed to write {}", temporary.display()))?;
        fs::rename(&temporary, &self.path).with_context(|| {
            format!(
                "Failed to replace {} with {}",
                self.path.display(),
                temporary.display()
            )
        })?;
        Ok(())
    }
}

fn default_managed_files() -> Vec<PathBuf> {
    vec![
        config::resolve_path("/etc/modprobe.d/tuned.conf"),
        config::resolve_path("/etc/systemd/system.conf.d/00-tuned.conf"),
        config::resolve_path("/etc/sysconfig/irqbalance"),
        config::resolve_path("/etc/tuned/bootcmdline"),
    ]
}

fn normalize_order(state: &mut RollbackState) {
    let mut seen = HashSet::new();
    state
        .order
        .retain(|key| state.entries.contains_key(key) && seen.insert(key.clone()));

    let mut missing = state
        .entries
        .keys()
        .filter(|key| !seen.contains(*key))
        .cloned()
        .collect::<Vec<_>>();
    missing.sort_unstable();
    state.order.extend(missing);
}

fn restore_entry(key: &str, original: &str, managed_files: &[PathBuf]) -> Result<()> {
    let (kind, target) = key
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("Invalid rollback key '{key}'"))?;

    match kind {
        "sysctl" => crate::tuning::sysctl::write_raw(target, original),
        "vm" => crate::tuning::vm::write_raw(target, original),
        "sysfs" => crate::tuning::sysfs::write_raw(Path::new(target), original),
        "file" => {
            restore_managed_file(Path::new(target), original, managed_files)?;
            if Path::new(target) == config::resolve_path("/etc/sysconfig/irqbalance") {
                crate::tuning::irqbalance::try_restart()?;
            }
            if Path::new(target) == config::resolve_path("/etc/tuned/bootcmdline") {
                crate::tuning::bootloader::sync_from_bootcmdline(Path::new(target))?;
            }
            Ok(())
        }
        "bootfile" => {
            validate_boot_file(Path::new(target))?;
            restore_file_snapshot(Path::new(target), original)
        }
        "grubfile" => {
            validate_grub_file(Path::new(target))?;
            restore_file_snapshot(Path::new(target), original)
        }
        "systemdfile" => {
            validate_systemd_dropin(Path::new(target))?;
            restore_file_snapshot(Path::new(target), original)?;
            crate::tuning::service::daemon_reload()
        }
        "service" => crate::tuning::service::restore_state(target, original),
        "mount-barrier" => crate::tuning::mounts::restore_barrier(target, original),
        "script" => crate::tuning::script::run_rollback_script(Path::new(target), original),
        "device-script-pre" | "device-script-post" => {
            crate::tuning::script::run_device_rollback(Path::new(target), original)
        }
        "hdparm-apm" => crate::tuning::disk::restore_hdparm("apm", target, original),
        "hdparm-spindown" => crate::tuning::disk::restore_hdparm("spindown", target, original),
        "net-channels" => crate::tuning::network::restore_channels(target, original),
        "net-ethtool" => crate::tuning::network::restore_ethtool(target, original),
        "net-advertise" => crate::tuning::network::restore_advertise(target, original),
        "irq-affinity" => crate::tuning::irq::write_raw(target, original),
        _ => bail!("Unknown rollback key type in '{key}'"),
    }
}

fn restore_managed_file(path: &Path, encoded: &str, managed_files: &[PathBuf]) -> Result<()> {
    validate_managed_file(path, managed_files)?;
    restore_file_snapshot(path, encoded)
}

fn restore_file_snapshot(path: &Path, encoded: &str) -> Result<()> {
    let snapshot: ManagedFileSnapshot = serde_json::from_str(encoded)
        .with_context(|| format!("Invalid file rollback snapshot for {}", path.display()))?;
    if snapshot.existed {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        let temporary = path.with_extension("tuned-rs-rollback");
        fs::write(&temporary, snapshot.contents)
            .with_context(|| format!("Failed to write {}", temporary.display()))?;
        fs::rename(&temporary, path)
            .with_context(|| format!("Failed to restore {}", path.display()))?;
    } else if path.exists() {
        fs::remove_file(path).with_context(|| format!("Failed to remove {}", path.display()))?;
    }
    Ok(())
}

fn validate_boot_file(path: &Path) -> Result<()> {
    let boot = config::resolve_path("/boot");
    if path.parent() == Some(boot.as_path())
        && path.file_name().is_some()
        && path.components().count() == boot.components().count() + 1
    {
        Ok(())
    } else {
        bail!(
            "Refusing boot-file rollback outside /boot: {}",
            path.display()
        )
    }
}

fn validate_grub_file(path: &Path) -> Result<()> {
    let boot = config::resolve_path("/boot");
    let etc_grub = [
        config::resolve_path("/etc/grub2.cfg"),
        config::resolve_path("/etc/grub2-efi.cfg"),
    ];
    let below_boot = path.strip_prefix(&boot).is_ok_and(|relative| {
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
    });
    if below_boot || etc_grub.iter().any(|candidate| candidate == path) {
        Ok(())
    } else {
        bail!(
            "Refusing GRUB-file rollback outside bootloader roots: {}",
            path.display()
        )
    }
}

fn validate_systemd_dropin(path: &Path) -> Result<()> {
    let root = config::resolve_path("/etc/systemd/system");
    let Ok(relative) = path.strip_prefix(&root) else {
        bail!("Refusing systemd rollback outside system unit configuration");
    };
    let components = relative.components().collect::<Vec<_>>();
    let directory = components
        .first()
        .and_then(|component| component.as_os_str().to_str())
        .unwrap_or_default();
    if components.len() == 2
        && directory.ends_with(".service.d")
        && components[1]
            .as_os_str()
            .to_str()
            .is_some_and(|name| !name.is_empty())
    {
        Ok(())
    } else {
        bail!("Invalid systemd service drop-in path: {}", path.display())
    }
}

fn validate_managed_file(path: &Path, allowed: &[PathBuf]) -> Result<()> {
    if allowed.iter().any(|candidate| candidate == path) {
        Ok(())
    } else {
        bail!(
            "Refusing managed-file rollback outside the TuneD allowlist: {}",
            path.display()
        )
    }
}

pub fn rollback_key(kind: &str, target: &str) -> String {
    format!("{kind}:{target}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn restores_in_reverse_order_and_retains_failures() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("rollback.json");
        let rollback = Rollback::load_from_path(path.clone()).unwrap();

        rollback.record_original("sysfs:first", "1").unwrap();
        rollback.record_original("sysfs:second", "2").unwrap();
        rollback.record_original("sysfs:third", "3").unwrap();

        let mut restored = Vec::new();
        let error = rollback
            .restore_with(|key, _| {
                restored.push(key.to_string());
                if key == "sysfs:second" {
                    bail!("device disappeared");
                }
                Ok(())
            })
            .unwrap_err();

        assert_eq!(restored, vec!["sysfs:third", "sysfs:second", "sysfs:first"]);
        assert!(error.to_string().contains("sysfs:second"));

        let persisted: RollbackFile =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(persisted.entries.len(), 1);
        assert_eq!(
            persisted.entries.get("sysfs:second"),
            Some(&"2".to_string())
        );
        assert_eq!(persisted.order, vec!["sysfs:second"]);

        rollback.restore_with(|_, _| Ok(())).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn loads_legacy_unordered_rollback_files() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("rollback.json");
        fs::write(&path, r#"{"entries":{"sysfs:b":"2","sysfs:a":"1"}}"#).unwrap();

        let rollback = Rollback::load_from_path(path).unwrap();
        let state = rollback.state.lock().unwrap();
        assert_eq!(state.order, vec!["sysfs:a", "sysfs:b"]);
    }

    #[test]
    fn restores_existing_managed_file_contents() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("etc/modprobe.d/tuned.conf");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"before\n").unwrap();
        let rollback = Rollback::load_from_path_with_managed_files(
            dir.path().join("rollback.json"),
            vec![path.clone()],
        )
        .unwrap();

        rollback.record_managed_file(&path).unwrap();
        fs::write(&path, b"after\n").unwrap();
        rollback.restore_all().unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"before\n");
    }

    #[test]
    fn removes_managed_file_that_did_not_exist_before_apply() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("etc/modprobe.d/tuned.conf");
        let rollback = Rollback::load_from_path_with_managed_files(
            dir.path().join("rollback.json"),
            vec![path.clone()],
        )
        .unwrap();

        rollback.record_managed_file(&path).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"created\n").unwrap();
        rollback.restore_all().unwrap();

        assert!(!path.exists());
    }

    #[test]
    fn rejects_managed_files_outside_the_pinned_allowlist() {
        let dir = TempDir::new().unwrap();
        let allowed = dir.path().join("etc/modprobe.d/tuned.conf");
        let rejected = dir.path().join("etc/shadow");
        let rollback = Rollback::load_from_path_with_managed_files(
            dir.path().join("rollback.json"),
            vec![allowed],
        )
        .unwrap();

        assert!(rollback.record_managed_file(&rejected).is_err());
    }
}
