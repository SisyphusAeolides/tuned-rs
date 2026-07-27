use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::config;
use crate::instances::InstanceRegistry;
use crate::profile::{self, Profile, ProfileCatalog};
use crate::rollback::Rollback;
use crate::socket_signals::SignalRegistry;
use crate::tuning;

pub struct Daemon {
    catalog: Mutex<ProfileCatalog>,
    rollback: Arc<Rollback>,
    active_profile: Mutex<String>,
    running: Mutex<bool>,
    manual: Mutex<bool>,
    instances: Mutex<InstanceRegistry>,
    signal_paths: Mutex<SignalRegistry>,
}

impl Daemon {
    pub fn new(catalog: ProfileCatalog, rollback: Arc<Rollback>) -> Arc<Self> {
        Arc::new(Self {
            catalog: Mutex::new(catalog),
            rollback,
            active_profile: Mutex::new(String::new()),
            running: Mutex::new(false),
            manual: Mutex::new(true),
            instances: Mutex::new(InstanceRegistry::default()),
            signal_paths: Mutex::new(SignalRegistry::default()),
        })
    }

    pub fn rollback(&self) -> Arc<Rollback> {
        self.rollback.clone()
    }

    pub async fn recover_previous_state(&self) -> Result<()> {
        let rollback = self.rollback.clone();
        run_blocking(move || rollback.restore_all())
    }

    pub async fn reload_catalog(&self) -> Result<()> {
        let dirs: Vec<_> = config::profile_dirs_from_env()
            .into_iter()
            .map(config::resolve_path_buf)
            .collect();
        let catalog = ProfileCatalog::load_from_dirs(&dirs)?;
        *self.catalog.lock().await = catalog;
        Ok(())
    }

    pub async fn start(&self) -> Result<bool> {
        if *self.running.lock().await {
            return Ok(true);
        }

        self.recover_previous_state().await?;

        let profile_name = match profile::read_active_profile() {
            Ok(Some(name)) => name,
            Ok(None) => config::DEFAULT_PROFILE.to_string(),
            Err(error) => {
                warn!("Could not read active profile: {error}");
                config::DEFAULT_PROFILE.to_string()
            }
        };

        match self.apply_profile(&profile_name, true).await {
            Ok(()) => {
                *self.running.lock().await = true;
                Ok(true)
            }
            Err(error) => {
                warn!("Failed to apply startup profile '{profile_name}': {error}");
                *self.running.lock().await = false;
                Ok(false)
            }
        }
    }

    pub async fn stop(&self, rollback: bool) -> bool {
        let mut success = true;
        if rollback && config::rollback_on_exit() {
            if let Err(error) = self.recover_previous_state().await {
                warn!("Failed to rollback on stop: {error}");
                success = false;
            }
        }
        *self.running.lock().await = false;
        success
    }

    pub async fn is_running(&self) -> bool {
        *self.running.lock().await
    }

    pub async fn active_profile(&self) -> String {
        self.active_profile.lock().await.clone()
    }

    pub async fn profile_mode(&self) -> (String, String) {
        let manual = *self.manual.lock().await;
        let mode = if manual { "manual" } else { "auto" };
        (mode.to_string(), String::new())
    }

    pub async fn post_loaded_profile(&self) -> String {
        self.active_profile().await
    }

    pub async fn profiles(&self) -> Vec<String> {
        self.catalog.lock().await.names()
    }

    pub async fn profiles2(&self) -> Vec<(String, String)> {
        self.catalog.lock().await.summaries()
    }

    pub async fn profile_info(&self, name: &str) -> (bool, String, String, String) {
        self.catalog.lock().await.profile_info(name)
    }

    pub async fn recommend_profile(&self) -> String {
        self.catalog.lock().await.recommend()
    }

    pub async fn switch_profile(&self, profile_name: &str, manual: bool) -> (bool, String) {
        let Some(normalized) = crate::engine::normalize_profile_selection(profile_name) else {
            return (false, "Invalid profile_name".to_string());
        };

        match self.apply_profile(&normalized, manual).await {
            Ok(()) => (true, "OK".to_string()),
            Err(error) => (false, error.to_string()),
        }
    }

    pub async fn apply_profile(&self, profile_name: &str, manual: bool) -> Result<()> {
        let profile = {
            let catalog = self.catalog.lock().await;
            catalog
                .resolve(profile_name)
                .with_context(|| format!("Profile selection '{profile_name}' not found"))?
        };
        let normalized = profile.name.clone();

        info!("Applying profile '{normalized}'");
        self.apply_profile_data(profile).await?;
        profile::save_active_profile(&normalized)?;
        profile::save_profile_mode(manual)?;
        *self.active_profile.lock().await = normalized;
        *self.manual.lock().await = manual;
        Ok(())
    }

    pub async fn reapply_active_profile(&self) -> Result<()> {
        let profile_name = self.active_profile.lock().await.clone();
        if profile_name.is_empty() {
            bail!("No active profile to reapply");
        }
        let manual = *self.manual.lock().await;
        self.apply_profile(&profile_name, manual).await
    }

    pub async fn disable(&self) -> bool {
        let stopped = self.stop(true).await;
        let cleared = match std::fs::write(config::resolve_path(config::ACTIVE_PROFILE_FILE), b"") {
            Ok(()) => true,
            Err(error) => {
                warn!("Failed to clear active profile: {error}");
                false
            }
        };
        *self.active_profile.lock().await = String::new();
        stopped && cleared
    }

    pub async fn instance_create(
        &self,
        plugin_name: &str,
        instance_name: &str,
        options: HashMap<String, String>,
    ) -> (bool, String) {
        self.instances
            .lock()
            .await
            .create(plugin_name, instance_name, options)
    }

    pub async fn instance_acquire_devices(
        &self,
        devices: &str,
        instance_name: &str,
    ) -> (bool, String) {
        self.instances
            .lock()
            .await
            .acquire_devices(devices, instance_name)
    }

    pub async fn get_instances(&self, plugin_name: &str) -> Vec<(String, String)> {
        self.instances.lock().await.list(plugin_name)
    }

    pub async fn instance_get_devices(&self, instance_name: &str) -> Option<Vec<String>> {
        self.instances.lock().await.devices(instance_name)
    }

    pub async fn instance_destroy(&self, instance_name: &str) -> (bool, String) {
        self.instances.lock().await.destroy(instance_name)
    }

    pub async fn register_socket_signal_path(&self, path: &str) -> bool {
        self.signal_paths.lock().await.register(path)
    }

    pub async fn emit_profile_changed(&self, profile_name: &str, result: bool, error: &str) {
        let registry = self.signal_paths.lock().await.clone();
        let profile_name = profile_name.to_string();
        let error = error.to_string();
        let failures = run_blocking(move || {
            Ok(registry.emit_profile_changed(&profile_name, result, &error))
        });
        match failures {
            Ok(failures) => {
                for (path, error) in failures {
                    warn!(
                        "Failed to send profile_changed signal to {}: {error}",
                        path.display()
                    );
                }
            }
            Err(error) => warn!("Failed to deliver profile_changed socket signals: {error}"),
        }
    }

    async fn apply_profile_data(&self, profile: Profile) -> Result<()> {
        let rollback = self.rollback.clone();
        run_blocking(move || {
            rollback.restore_all()?;
            if let Err(error) = tuning::apply_profile(&rollback, &profile) {
                let rollback_result = rollback.restore_all();
                return match rollback_result {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(anyhow::anyhow!(
                        "{error}; rollback after failed apply also failed: {rollback_error}"
                    )),
                };
            }
            Ok(())
        })
    }
}

fn run_blocking<F, T>(work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    std::thread::spawn(work)
        .join()
        .map_err(|_| anyhow::anyhow!("Blocking task panicked"))?
}
