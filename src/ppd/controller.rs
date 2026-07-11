use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{Mutex, RwLock, mpsc};
use tracing::{debug, error, info, warn};
use zbus::names::BusName;
use zbus::{Connection, MatchRule, MessageStream};

use crate::ppd::config::{
    self, platform_profile_to_ppd, PpdConfig, PERFORMANCE, POWER_SAVER, UNKNOWN_PROFILE,
};
use crate::ppd::ipc;
use crate::ppd::tuned_client::{ProfileChangedEvent, TunedClient};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PerformanceDegraded {
    None,
    LapDetected,
    HighOperatingTemperature,
}

impl PerformanceDegraded {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "",
            Self::LapDetected => "lap-detected",
            Self::HighOperatingTemperature => "high-operating-temperature",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProfileHold {
    pub profile: String,
    pub reason: String,
    pub app_id: String,
    pub caller: String,
}

pub struct PpdCore {
    pub config: RwLock<PpdConfig>,
    pub active_profile: RwLock<String>,
    pub base_profile: RwLock<String>,
    pub performance_degraded: RwLock<PerformanceDegraded>,
    pub holds: Mutex<HashMap<u32, ProfileHold>>,
    pub on_battery: RwLock<bool>,
    tuned: TunedClient,
    connection: Connection,
    cookie_generator: AtomicU32,
    hold_release_tx: mpsc::Sender<u32>,
}

impl PpdCore {
    pub async fn new(
        connection: Connection,
        tuned: TunedClient,
        hold_release_tx: mpsc::Sender<u32>,
    ) -> Result<Arc<Self>> {
        let profiles = tuned.profiles().await?;
        let config = PpdConfig::load(config::CONFIG_FILE, &profiles)?;
        Ok(Arc::new(Self {
            config: RwLock::new(config),
            active_profile: RwLock::new(String::new()),
            base_profile: RwLock::new(String::new()),
            performance_degraded: RwLock::new(PerformanceDegraded::None),
            holds: Mutex::new(HashMap::new()),
            on_battery: RwLock::new(false),
            tuned,
            connection,
            cookie_generator: AtomicU32::new(1),
            hold_release_tx,
        }))
    }

    pub async fn initialize(&self) -> Result<()> {
        let profiles = self.tuned.profiles().await?;
        let config = PpdConfig::load(config::CONFIG_FILE, &profiles)?;
        *self.config.write().await = config;

        self.check_performance_degraded().await;

        if self.config.read().await.battery_detection {
            if let Err(error) = self.refresh_battery_state().await {
                warn!("Battery detection unavailable: {error}");
            }
        } else {
            *self.on_battery.write().await = false;
        }

        let default_profile = self.config.read().await.default_profile.clone();
        let base = read_base_profile()
            .await
            .unwrap_or(default_profile);
        self.switch_profile(&base).await?;
        let _ = self.set_base_profile(&base).await;
        Ok(())
    }

    pub async fn active_profile(&self) -> String {
        self.active_profile.read().await.clone()
    }

    pub async fn performance_degraded(&self) -> PerformanceDegraded {
        self.performance_degraded.read().await.clone()
    }

    pub async fn active_profile_holds(&self) -> Vec<HashMap<String, String>> {
        self.holds
            .lock()
            .await
            .values()
            .map(|hold| {
                HashMap::from([
                    ("Profile".to_string(), hold.profile.clone()),
                    ("Reason".to_string(), hold.reason.clone()),
                    ("ApplicationId".to_string(), hold.app_id.clone()),
                ])
            })
            .collect()
    }

    pub async fn profiles(&self) -> Vec<HashMap<String, String>> {
        let config = self.config.read().await;
        let on_battery = *self.on_battery.read().await;
        config
            .ppd_to_tuned
            .keys(on_battery)
            .map(|profile| {
                HashMap::from([
                    ("Profile".to_string(), profile.clone()),
                    ("Driver".to_string(), config::DRIVER.to_string()),
                ])
            })
            .collect()
    }

    pub async fn set_active_profile(&self, profile: &str) -> Result<()> {
        let config = self.config.read().await;
        let on_battery = *self.on_battery.read().await;
        if config
            .ppd_to_tuned
            .get(profile, on_battery)
            .is_err()
        {
            anyhow::bail!("Invalid profile '{profile}'");
        }
        drop(config);

        self.clear_holds().await;
        self.switch_profile(profile).await?;
        let _ = self.set_base_profile(profile).await;
        Ok(())
    }

    pub async fn switch_profile(&self, profile: &str) -> Result<()> {
        let tuned_profile = {
            let config = self.config.read().await;
            let on_battery = *self.on_battery.read().await;
            config.ppd_to_tuned.get(profile, on_battery)?.to_string()
        };

        if !self.set_tuned_profile(&tuned_profile).await? {
            anyhow::bail!("Error setting profile '{profile}'");
        }

        let mut active = self.active_profile.write().await;
        if *active != profile {
            info!("Profile changed to '{profile}'");
            *active = profile.to_string();
        }
        Ok(())
    }

    async fn set_tuned_profile(&self, tuned_profile: &str) -> Result<bool> {
        let current = self.tuned.active_profile().await?;
        if current == tuned_profile {
            return Ok(true);
        }
        info!("Setting TuneD profile to '{tuned_profile}'");
        let (ok, message) = self.tuned.switch_profile(tuned_profile).await?;
        if !ok {
            error!("{message}");
        }
        Ok(ok)
    }

    pub async fn set_base_profile(&self, profile: &str) -> Result<()> {
        *self.base_profile.write().await = profile.to_string();
        std::fs::write(config::BASE_PROFILE_FILE, format!("{profile}\n"))
            .with_context(|| format!("Failed to write {}", config::BASE_PROFILE_FILE))?;
        Ok(())
    }

    pub async fn base_profile(&self) -> String {
        self.base_profile.read().await.clone()
    }

    pub async fn add_hold(
        self: &Arc<Self>,
        profile: &str,
        reason: &str,
        app_id: &str,
        caller: &str,
    ) -> Result<u32> {
        if profile != POWER_SAVER && profile != PERFORMANCE {
            anyhow::bail!("Only '{POWER_SAVER}' and '{PERFORMANCE}' profiles may be held");
        }

        let cookie = loop {
            let cookie = self.cookie_generator.fetch_add(1, Ordering::Relaxed);
            if cookie != 0 && !self.holds.lock().await.contains_key(&cookie) {
                break cookie;
            }
        };

        info!("Adding hold '{cookie}': profile '{profile}' by application '{app_id}'");
        self.holds.lock().await.insert(
            cookie,
            ProfileHold {
                profile: profile.to_string(),
                reason: reason.to_string(),
                app_id: app_id.to_string(),
                caller: caller.to_string(),
            },
        );

        self.watch_hold_owner(cookie, caller.to_string(), app_id.to_string());
        self.switch_profile(&self.effective_hold_profile().await)
            .await?;
        Ok(cookie)
    }

    pub async fn release_hold(&self, cookie: u32, caller: &str) -> Result<()> {
        let hold = self
            .holds
            .lock()
            .await
            .get(&cookie)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No active hold for cookie '{cookie}'"))?;
        if hold.caller != caller {
            anyhow::bail!("Cannot release a profile hold initiated by another process.");
        }
        self.remove_hold(cookie).await?;
        Ok(())
    }

    pub async fn remove_hold(&self, cookie: u32) -> Result<u32> {
        let hold = self.holds.lock().await.remove(&cookie);
        if let Some(hold) = hold {
            info!(
                "Releasing hold '{cookie}': profile '{}' by application '{}'",
                hold.profile, hold.app_id
            );
            let _ = self.hold_release_tx.send(cookie).await;
        }
        let next = if self.holds.lock().await.is_empty() {
            self.base_profile().await
        } else {
            self.effective_hold_profile().await
        };
        self.switch_profile(&next).await?;
        Ok(cookie)
    }

    async fn effective_hold_profile(&self) -> String {
        let holds = self.holds.lock().await;
        if holds
            .values()
            .any(|hold| hold.profile == POWER_SAVER)
        {
            POWER_SAVER.to_string()
        } else {
            PERFORMANCE.to_string()
        }
    }

    async fn clear_holds(&self) {
        self.holds.lock().await.clear();
    }

    fn watch_hold_owner(self: &Arc<Self>, cookie: u32, caller: String, app_id: String) {
        let core = Arc::clone(self);
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    warn!("Failed to start hold monitor thread: {error}");
                    return;
                }
            };
            runtime.block_on(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if !core.holds.lock().await.contains_key(&cookie) {
                        break;
                    }
                    let proxy = match zbus::Proxy::new(
                        &core.connection,
                        "org.freedesktop.DBus",
                        "/org/freedesktop/DBus",
                        "org.freedesktop.DBus",
                    )
                    .await
                    {
                        Ok(proxy) => proxy,
                        Err(error) => {
                            warn!("Hold monitor DBus error: {error}");
                            continue;
                        }
                    };
                    let has_owner: bool =
                        match proxy.call("NameHasOwner", &(caller.as_str(),)).await {
                            Ok(value) => value,
                            Err(error) => {
                                warn!("NameHasOwner failed for '{caller}': {error}");
                                continue;
                            }
                        };
                    if !has_owner {
                        info!("Application '{app_id}' disappeared, releasing hold '{cookie}'");
                        let _ = core.remove_hold(cookie).await;
                        break;
                    }
                }
            });
        });
    }

    pub async fn handle_tuned_profile_changed(&self, event: ProfileChangedEvent) {
        if !event.result {
            return;
        }

        let tuned_profile = match self.tuned.active_profile().await {
            Ok(profile) => profile,
            Err(error) => {
                warn!("Failed to read active TuneD profile: {error}");
                return;
            }
        };

        if event.profile_name != tuned_profile {
            debug!("Ignoring stale TuneD profile_changed signal");
            return;
        }

        let on_battery = *self.on_battery.read().await;
        let ppd_profile = self
            .config
            .read()
            .await
            .tuned_to_ppd
            .reverse_lookup(&tuned_profile, on_battery)
            .unwrap_or(UNKNOWN_PROFILE)
            .to_string();

        let current = self.active_profile().await;
        if ppd_profile == UNKNOWN_PROFILE {
            let on_battery = *self.on_battery.read().await;
            if self
                .config
                .read()
                .await
                .ppd_to_tuned
                .get(&current, on_battery)
                .map(|expected| expected == tuned_profile)
                .unwrap_or(false)
            {
                return;
            }
            warn!("TuneD profile changed to unknown profile '{tuned_profile}'");
        }

        if current != ppd_profile {
            info!("Profile changed to '{ppd_profile}'");
            *self.active_profile.write().await = ppd_profile.clone();
            self.clear_holds().await;
            if ppd_profile != UNKNOWN_PROFILE {
                let _ = self.set_base_profile(&ppd_profile).await;
            }
        }
    }

    pub async fn on_battery_changed(&self) {
        let on_battery = *self.on_battery.read().await;
        info!(
            "Battery status changed: {}",
            if on_battery {
                "DC (battery)"
            } else {
                "AC (charging)"
            }
        );
        let active = self.active_profile().await;
        let _ = self.switch_profile(&active).await;
    }

    pub async fn check_performance_degraded(&self) {
        let mut degraded = PerformanceDegraded::None;
        if let Ok(value) = read_trimmed(config::NO_TURBO_PATH) {
            if value == "1" {
                degraded = PerformanceDegraded::HighOperatingTemperature;
            }
        }
        if let Ok(value) = read_trimmed(config::LAP_MODE_PATH) {
            if value == "1" {
                degraded = PerformanceDegraded::LapDetected;
            }
        }

        let mut current = self.performance_degraded.write().await;
        if *current != degraded {
            info!("Performance degraded: {}", degraded.as_str());
            *current = degraded;
        }
    }

    pub async fn check_platform_profile(&self) -> Result<()> {
        let platform = read_trimmed(config::PLATFORM_PROFILE_PATH)?;
        let Some(ppd_profile) = platform_profile_to_ppd(&platform) else {
            return Ok(());
        };
        info!("Platform profile changed: {platform}");

        let active = self.active_profile().await;
        let base = self.base_profile().await;
        if !base.is_empty() && active == base && ppd_profile != base {
            debug!(
                "Ignoring platform profile '{platform}' because the user-selected base profile is '{base}'"
            );
            return Ok(());
        }

        self.clear_holds().await;
        self.switch_profile(ppd_profile).await
    }

    pub async fn refresh_battery_state(&self) -> Result<()> {
        let proxy = zbus::Proxy::new(
            &self.connection,
            config::UPOWER_POWER_BUS,
            config::UPOWER_POWER_PATH,
            config::UPOWER_POWER_BUS,
        )
        .await?;
        let on_battery: bool = proxy.get_property("OnBattery").await?;
        *self.on_battery.write().await = on_battery;
        Ok(())
    }

    pub fn tuned(&self) -> &TunedClient {
        &self.tuned
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

async fn read_base_profile() -> Option<String> {
    match std::fs::read_to_string(config::BASE_PROFILE_FILE) {
        Ok(content) => {
            let profile = content.trim();
            if profile.is_empty() {
                None
            } else {
                Some(profile.to_string())
            }
        }
        Err(_) => None,
    }
}

fn read_trimmed(path: &str) -> Result<String> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {path}"))?;
    Ok(content.trim().to_string())
}

pub async fn run() -> Result<()> {
    let connection = Connection::system().await?;
    let tuned = TunedClient::connect(&connection).await?;
    let (hold_release_tx, hold_release_rx) = mpsc::channel(8);
    let core = PpdCore::new(connection.clone(), tuned, hold_release_tx).await?;
    core.initialize().await?;

    let core_for_tasks = core.clone();
    tokio::spawn(async move {
        if let Err(error) = spawn_tuned_signal_task(core_for_tasks).await {
            error!("TuneD profile_changed listener failed: {error}");
        }
    });

    let core_for_battery = core.clone();
    tokio::spawn(async move {
        if let Err(error) = spawn_battery_monitor(core_for_battery).await {
            warn!("Battery monitor stopped: {error}");
        }
    });

    let core_for_perf = core.clone();
    tokio::spawn(async move {
        spawn_performance_monitor(core_for_perf).await;
    });

    if core.config.read().await.sysfs_acpi_monitor
        && std::path::Path::new(config::PLATFORM_PROFILE_PATH).exists()
    {
        let core_for_platform = core.clone();
        tokio::spawn(async move {
            spawn_platform_profile_monitor(core_for_platform).await;
        });
    }

    let _dbus_conn = ipc::spawn_servers(core, hold_release_rx).await?;
    tokio::signal::ctrl_c().await?;
    Ok(())
}

async fn spawn_tuned_signal_task(core: Arc<PpdCore>) -> Result<()> {
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(BusName::try_from("com.redhat.tuned")?)?
        .path("/Tuned")?
        .interface("com.redhat.tuned.control")?
        .member("profile_changed")?
        .build();

    let mut stream = MessageStream::for_match_rule(rule, core.connection(), None).await?;
    use futures_util::StreamExt;
    while let Some(message) = stream.next().await {
        let message = message?;
        let body = message.body();
        let (profile_name, result, errstr): (String, bool, String) = body.deserialize()?;
        core.handle_tuned_profile_changed(ProfileChangedEvent {
            profile_name,
            result,
            errstr,
        })
        .await;
    }
    Ok(())
}

async fn spawn_battery_monitor(core: Arc<PpdCore>) -> Result<()> {
    if !core.config.read().await.battery_detection {
        return Ok(());
    }

    core.refresh_battery_state().await?;

    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(BusName::try_from(config::UPOWER_POWER_BUS)?)?
        .path(config::UPOWER_POWER_PATH)?
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .build();

    let mut stream = MessageStream::for_match_rule(rule, core.connection(), None).await?;
    use futures_util::StreamExt;
    while let Some(message) = stream.next().await {
        let message = message?;
        let body = message.body();
        let args: (String, HashMap<String, zbus::zvariant::OwnedValue>, Vec<String>) =
            body.deserialize()?;
        if args.0 == config::UPOWER_POWER_BUS {
            if let Some(value) = args.1.get("OnBattery") {
                if let Ok(on_battery) = bool::try_from(value) {
                    *core.on_battery.write().await = on_battery;
                    core.on_battery_changed().await;
                }
            }
        }
    }
    Ok(())
}

async fn spawn_performance_monitor(core: Arc<PpdCore>) {
    let paths = [config::NO_TURBO_PATH, config::LAP_MODE_PATH];
    let existing: Vec<_> = paths
        .iter()
        .filter(|path| std::path::Path::new(path).exists())
        .copied()
        .collect();

    if existing.is_empty() {
        return;
    }

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        interval.tick().await;
        core.check_performance_degraded().await;
    }
}

async fn spawn_platform_profile_monitor(core: Arc<PpdCore>) {
    let path = config::PLATFORM_PROFILE_PATH;
    let mut last = read_trimmed(path).unwrap_or_default();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        interval.tick().await;
        if let Ok(current) = read_trimmed(path) {
            if current != last {
                last = current.clone();
                if let Err(error) = core.check_platform_profile().await {
                    warn!("Failed to apply platform profile '{current}': {error}");
                }
            }
        }
    }
}
