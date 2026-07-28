use std::collections::{BTreeSet, HashMap};
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
use crate::verification;
use crate::verification_sysfs;
use crate::verification_units;

pub struct Daemon {
    catalog: Mutex<ProfileCatalog>,
    rollback: Arc<Rollback>,
    active_profile: Mutex<String>,
    running: Mutex<bool>,
    manual: Mutex<bool>,
    instances: Mutex<InstanceRegistry>,
    instance_rollbacks: Mutex<HashMap<String, Arc<Rollback>>>,
    instance_order: Mutex<Vec<String>>,
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
            instance_rollbacks: Mutex::new(HashMap::new()),
            instance_order: Mutex::new(Vec::new()),
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
        if let Err(error) = self.destroy_all_dynamic_instances().await {
            warn!("Failed to rollback dynamic instances on stop: {error}");
            success = false;
        }
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

    pub async fn verify_active_profile(&self, ignore_missing: bool) -> bool {
        let profile_name = self.active_profile.lock().await.clone();
        if profile_name.is_empty() {
            warn!("Cannot verify a profile because no profile is active");
            return false;
        }

        let profile = {
            let catalog = self.catalog.lock().await;
            catalog.resolve(&profile_name)
        };
        let Some(profile) = profile else {
            warn!("Cannot resolve active profile selection '{profile_name}' for verification");
            return false;
        };

        match run_blocking(move || {
            let mut report = verification::verify_profile(&profile);
            verification_units::augment(&profile, &mut report);
            verification_sysfs::augment(&profile, &mut report);
            Ok(report)
        }) {
            Ok(report) => {
                report.log();
                report.passes(ignore_missing)
            }
            Err(error) => {
                warn!("Profile verification failed to run: {error}");
                false
            }
        }
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
        let result = self
            .instances
            .lock()
            .await
            .create(plugin_name, instance_name, options);
        if !result.0 {
            return result;
        }
        let instance = self
            .instances
            .lock()
            .await
            .instance(instance_name)
            .expect("newly created instance must exist");
        let unit = match dynamic_instance_unit(&instance) {
            Ok(unit) => unit,
            Err(error) => {
                self.instances.lock().await.destroy(instance_name);
                return (false, error.to_string());
            }
        };
        let rollback = match Rollback::load_instance(instance_name) {
            Ok(rollback) => Arc::new(rollback),
            Err(error) => {
                self.instances.lock().await.destroy(instance_name);
                return (false, error.to_string());
            }
        };
        let transaction = Arc::clone(&rollback);
        let apply = run_blocking(move || {
            transaction.restore_all()?;
            if let Err(error) = tuning::apply_dynamic_unit(&transaction, &unit) {
                let _ = transaction.restore_all();
                return Err(error);
            }
            Ok(())
        });
        match apply {
            Ok(()) => {
                self.instance_rollbacks
                    .lock()
                    .await
                    .insert(instance_name.to_string(), rollback);
                self.instance_order
                    .lock()
                    .await
                    .push(instance_name.to_string());
                (true, "OK".to_string())
            }
            Err(error) => {
                self.instances.lock().await.destroy(instance_name);
                (
                    false,
                    format!("Error creating instance '{instance_name}': {error}"),
                )
            }
        }
    }

    pub async fn instance_acquire_devices(
        &self,
        devices: &str,
        instance_name: &str,
    ) -> (bool, String) {
        let before = self.instances.lock().await.clone();
        let result = self
            .instances
            .lock()
            .await
            .acquire_devices(devices, instance_name);
        if !result.0 {
            return result;
        }
        let after = self.instances.lock().await.clone();
        let changed = changed_instance_names(&before, &after);
        let order = self
            .instance_order
            .lock()
            .await
            .iter()
            .filter(|name| changed.contains(*name))
            .cloned()
            .collect::<Vec<_>>();
        let rollbacks = self.instance_rollbacks.lock().await;
        let mut transactions = Vec::new();
        for name in &order {
            let Some(rollback) = rollbacks.get(name).cloned() else {
                *self.instances.lock().await = before;
                return (
                    false,
                    format!("Missing rollback for dynamic instance '{name}'"),
                );
            };
            let old = before
                .instance(name)
                .and_then(|value| dynamic_instance_unit(&value).ok());
            let new = after
                .instance(name)
                .and_then(|value| dynamic_instance_unit(&value).ok());
            let (Some(old), Some(new)) = (old, new) else {
                *self.instances.lock().await = before;
                return (false, format!("Cannot rebuild dynamic instance '{name}'"));
            };
            transactions.push((rollback, old, new));
        }
        drop(rollbacks);
        let retune = run_blocking(move || retune_dynamic_instances(&transactions));
        if let Err(error) = retune {
            *self.instances.lock().await = before;
            return (false, format!("Dynamic device transfer failed: {error}"));
        }
        result
    }

    pub async fn get_instances(&self, plugin_name: &str) -> Vec<(String, String)> {
        self.instances.lock().await.list(plugin_name)
    }

    pub async fn instance_get_devices(&self, instance_name: &str) -> Option<Vec<String>> {
        self.instances.lock().await.devices(instance_name)
    }

    pub async fn instance_destroy(&self, instance_name: &str) -> (bool, String) {
        let rollback = self
            .instance_rollbacks
            .lock()
            .await
            .get(instance_name)
            .cloned();
        let Some(rollback) = rollback else {
            return self.instances.lock().await.destroy(instance_name);
        };
        if let Err(error) = run_blocking(move || rollback.restore_all()) {
            return (
                false,
                format!("Error deleting instance '{instance_name}': {error}"),
            );
        }
        self.instance_rollbacks.lock().await.remove(instance_name);
        self.instance_order
            .lock()
            .await
            .retain(|name| name != instance_name);
        self.instances.lock().await.destroy(instance_name)
    }

    pub async fn register_socket_signal_path(&self, path: &str) -> bool {
        self.signal_paths.lock().await.register(path)
    }

    pub async fn emit_profile_changed(&self, profile_name: &str, result: bool, error: &str) {
        let registry = self.signal_paths.lock().await.clone();
        let profile_name = profile_name.to_string();
        let error = error.to_string();
        let failures =
            run_blocking(move || Ok(registry.emit_profile_changed(&profile_name, result, &error)));
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
        self.destroy_all_dynamic_instances().await?;
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

    async fn destroy_all_dynamic_instances(&self) -> Result<()> {
        let order = self.instance_order.lock().await.clone();
        for name in order.into_iter().rev() {
            let rollback = self
                .instance_rollbacks
                .lock()
                .await
                .get(&name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Missing rollback for dynamic instance '{name}'"))?;
            run_blocking(move || rollback.restore_all())?;
        }
        self.instance_rollbacks.lock().await.clear();
        self.instance_order.lock().await.clear();
        *self.instances.lock().await = InstanceRegistry::default();
        Ok(())
    }
}

fn dynamic_instance_unit(
    instance: &crate::instances::DynamicInstance,
) -> Result<crate::profile_units::ProfileUnit> {
    let mut options = instance
        .options
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<Vec<_>>();
    options.push(("type".to_string(), instance.plugin.clone()));
    options.retain(|(name, _)| name != "devices");
    options.push((
        "devices".to_string(),
        if instance.devices.is_empty() {
            "!*".to_string()
        } else {
            instance
                .devices
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(",")
        },
    ));
    crate::profile_units::ProfileUnit::from_options(&instance.name, options)
}

fn changed_instance_names(before: &InstanceRegistry, after: &InstanceRegistry) -> BTreeSet<String> {
    let before = before
        .all_instances()
        .into_iter()
        .map(|instance| (instance.name.clone(), instance))
        .collect::<HashMap<_, _>>();
    let after = after
        .all_instances()
        .into_iter()
        .map(|instance| (instance.name.clone(), instance))
        .collect::<HashMap<_, _>>();
    before
        .keys()
        .chain(after.keys())
        .filter(|name| before.get(*name) != after.get(*name))
        .cloned()
        .collect()
}

fn retune_dynamic_instances(
    transactions: &[(
        Arc<Rollback>,
        crate::profile_units::ProfileUnit,
        crate::profile_units::ProfileUnit,
    )],
) -> Result<()> {
    for (rollback, _, _) in transactions.iter().rev() {
        rollback.restore_all()?;
    }
    for (index, (rollback, _, new)) in transactions.iter().enumerate() {
        if let Err(error) = tuning::apply_dynamic_unit(rollback, new) {
            for (rollback, _, _) in transactions[..=index].iter().rev() {
                let _ = rollback.restore_all();
            }
            for (rollback, old, _) in transactions {
                tuning::apply_dynamic_unit(rollback, old)?;
            }
            return Err(error);
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::await_holding_lock)]
    async fn dynamic_disk_instance_applies_and_restores_tuning() {
        let _env_guard = crate::config::test_env_lock();
        let root = TempDir::new().unwrap();
        let queue = root.path().join("sys/block/sda/queue");
        let profiles = root.path().join("profiles");
        std::fs::create_dir_all(&queue).unwrap();
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(queue.join("read_ahead_kb"), "128\n").unwrap();
        std::env::set_var("TUNED_RS_ROOT", root.path());

        let catalog = ProfileCatalog::load_from_dirs(&[profiles]).unwrap();
        let daemon = Daemon::new(catalog, Arc::new(Rollback::load().unwrap()));
        let options = HashMap::from([
            ("devices".to_string(), "sda".to_string()),
            ("readahead".to_string(), "256".to_string()),
        ]);
        assert_eq!(
            daemon.instance_create("disk", "interactive", options).await,
            (true, "OK".to_string())
        );
        assert_eq!(
            std::fs::read_to_string(queue.join("read_ahead_kb"))
                .unwrap()
                .trim(),
            "256"
        );
        assert_eq!(
            daemon.instance_destroy("interactive").await,
            (true, "OK".to_string())
        );
        assert_eq!(
            std::fs::read_to_string(queue.join("read_ahead_kb"))
                .unwrap()
                .trim(),
            "128"
        );

        let second_queue = root.path().join("sys/block/sdb/queue");
        std::fs::create_dir_all(&second_queue).unwrap();
        std::fs::write(second_queue.join("read_ahead_kb"), "128\n").unwrap();
        assert!(
            daemon
                .instance_create(
                    "disk",
                    "first",
                    HashMap::from([
                        ("devices".to_string(), "sda".to_string()),
                        ("readahead".to_string(), "256".to_string()),
                    ]),
                )
                .await
                .0
        );
        assert!(
            daemon
                .instance_create(
                    "disk",
                    "second",
                    HashMap::from([
                        ("devices".to_string(), "sdb".to_string()),
                        ("readahead".to_string(), "512".to_string()),
                    ]),
                )
                .await
                .0
        );
        assert_eq!(
            daemon.instance_acquire_devices("sda", "second").await,
            (true, "OK".to_string())
        );
        assert_eq!(
            std::fs::read_to_string(queue.join("read_ahead_kb"))
                .unwrap()
                .trim(),
            "512"
        );
        assert_eq!(daemon.instance_get_devices("first").await, Some(Vec::new()));
        assert_eq!(
            daemon.instance_get_devices("second").await,
            Some(vec!["sda".to_string(), "sdb".to_string()])
        );
        daemon.destroy_all_dynamic_instances().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(queue.join("read_ahead_kb"))
                .unwrap()
                .trim(),
            "128"
        );
        std::env::remove_var("TUNED_RS_ROOT");
    }
}
