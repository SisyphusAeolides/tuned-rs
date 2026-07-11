use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RollbackFile {
    entries: HashMap<String, String>,
}

pub struct Rollback {
    path: PathBuf,
    entries: Mutex<HashMap<String, String>>,
}

impl Rollback {
    pub fn load() -> Result<Self> {
        let path = config::resolve_path(config::ROLLBACK_FILE);
        let entries = if path.is_file() {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            serde_json::from_str::<RollbackFile>(&content)
                .with_context(|| format!("Failed to parse {}", path.display()))?
                .entries
        } else {
            HashMap::new()
        };

        if !entries.is_empty() {
            info!("Loaded {} rollback entries from disk", entries.len());
        }

        Ok(Self {
            path,
            entries: Mutex::new(entries),
        })
    }

    pub fn record_original(&self, key: &str, original: &str) -> Result<()> {
        let mut entries = self.entries.lock().unwrap();
        if !entries.contains_key(key) {
            entries.insert(key.to_string(), original.to_string());
            drop(entries);
            self.persist()?;
        }
        Ok(())
    }

    pub fn restore_all(&self) -> Result<()> {
        let entries: HashMap<_, _> = self.entries.lock().unwrap().clone();
        if entries.is_empty() {
            return Ok(());
        }

        info!("Restoring {} tuned value(s)", entries.len());
        for (key, original) in &entries {
            if let Err(error) = restore_entry(key, original) {
                warn!("Failed to restore '{key}': {error}");
            }
        }

        self.clear()?;
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        self.entries.lock().unwrap().clear();
        if self.path.is_file() {
            fs::remove_file(&self.path)
                .with_context(|| format!("Failed to remove {}", self.path.display()))?;
        }
        Ok(())
    }

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }

        let snapshot = RollbackFile {
            entries: self.entries.lock().unwrap().clone(),
        };
        let content = serde_json::to_string_pretty(&snapshot)?;
        fs::write(&self.path, content)
            .with_context(|| format!("Failed to write {}", self.path.display()))?;
        Ok(())
    }
}

fn restore_entry(key: &str, original: &str) -> Result<()> {
    let (kind, target) = key
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("Invalid rollback key '{key}'"))?;

    match kind {
        "sysctl" => crate::tuning::sysctl::write_raw(target, original),
        "vm" => crate::tuning::vm::write_raw(target, original),
        "sysfs" => crate::tuning::sysfs::write_raw(Path::new(target), original),
        _ => anyhow::bail!("Unknown rollback key type in '{key}'"),
    }
}

pub fn rollback_key(kind: &str, target: &str) -> String {
    format!("{kind}:{target}")
}
