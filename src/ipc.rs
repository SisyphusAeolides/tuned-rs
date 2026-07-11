use std::collections::HashMap;
use std::sync::Arc;

use tracing::{error, info};
use zbus::message::Header;
use zbus::{connection::Builder, interface, Connection, SignalContext};

use crate::config;
use crate::daemon::Daemon;
use crate::polkit::{self, Polkit};

pub struct TunedController {
    daemon: Arc<Daemon>,
    polkit: Polkit,
}

impl TunedController {
    async fn authorized(&self, header: &Header<'_>, action: &str) -> bool {
        self.polkit
            .authorize(polkit::sender_from_header(header).as_deref(), action)
            .await
    }
}

#[interface(name = "com.redhat.tuned.control")]
impl TunedController {
    #[zbus(signal, name = "profile_changed")]
    async fn profile_changed(
        ctxt: &SignalContext<'_>,
        profile_name: &str,
        result: bool,
        errstr: &str,
    ) -> zbus::Result<()>;

    #[zbus(name = "start")]
    async fn start(&self, #[zbus(header)] header: Header<'_>) -> bool {
        if !self.authorized(&header, "start").await {
            return false;
        }
        match self.daemon.start().await {
            Ok(result) => result,
            Err(error) => {
                error!("Failed to start daemon: {error}");
                false
            }
        }
    }

    #[zbus(name = "stop")]
    async fn stop(&self, #[zbus(header)] header: Header<'_>) -> bool {
        if !self.authorized(&header, "stop").await {
            return false;
        }
        self.daemon.stop(true).await
    }

    #[zbus(name = "reload")]
    async fn reload(&self, #[zbus(header)] header: Header<'_>) -> bool {
        if !self.authorized(&header, "reload").await {
            return false;
        }
        if !self.daemon.stop(false).await {
            return false;
        }
        if let Err(error) = self.daemon.reload_catalog().await {
            error!("Failed to reload profile catalog: {error}");
            return false;
        }
        self.daemon.start().await.unwrap_or(false)
    }

    #[zbus(name = "switch_profile")]
    async fn switch_profile(
        &self,
        profile_name: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> (bool, String) {
        if !self.authorized(&header, "switch_profile").await {
            return polkit::unauthorized_pair();
        }
        let result = self.daemon.switch_profile(profile_name, true).await;
        if let Err(error) =
            Self::profile_changed(&ctxt, profile_name, result.0, &result.1).await
        {
            error!("Failed to emit profile_changed signal: {error}");
        }
        result
    }

    #[zbus(name = "auto_profile")]
    async fn auto_profile(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) -> (bool, String) {
        if !self.authorized(&header, "auto_profile").await {
            return polkit::unauthorized_pair();
        }
        let profile = self.daemon.recommend_profile().await;
        let result = self.daemon.switch_profile(&profile, false).await;
        if let Err(error) = Self::profile_changed(&ctxt, &profile, result.0, &result.1).await {
            error!("Failed to emit profile_changed signal: {error}");
        }
        result
    }

    #[zbus(name = "active_profile")]
    async fn active_profile(&self, #[zbus(header)] header: Header<'_>) -> String {
        if !self.authorized(&header, "active_profile").await {
            return String::new();
        }
        self.daemon.active_profile().await
    }

    #[zbus(name = "profile_mode")]
    async fn profile_mode(&self, #[zbus(header)] header: Header<'_>) -> (String, String) {
        if !self.authorized(&header, "profile_mode").await {
            return ("unknown".to_string(), "Unauthorized".to_string());
        }
        self.daemon.profile_mode().await
    }

    #[zbus(name = "post_loaded_profile")]
    async fn post_loaded_profile(&self, #[zbus(header)] header: Header<'_>) -> String {
        if !self.authorized(&header, "post_loaded_profile").await {
            return String::new();
        }
        self.daemon.post_loaded_profile().await
    }

    #[zbus(name = "disable")]
    async fn disable(&self, #[zbus(header)] header: Header<'_>) -> bool {
        if !self.authorized(&header, "disable").await {
            return false;
        }
        self.daemon.disable().await
    }

    #[zbus(name = "is_running")]
    async fn is_running(&self, #[zbus(header)] header: Header<'_>) -> bool {
        if !self.authorized(&header, "is_running").await {
            return false;
        }
        self.daemon.is_running().await
    }

    #[zbus(name = "profiles")]
    async fn profiles(&self, #[zbus(header)] header: Header<'_>) -> Vec<String> {
        if !self.authorized(&header, "profiles").await {
            return Vec::new();
        }
        self.daemon.profiles().await
    }

    #[zbus(name = "profiles2")]
    async fn profiles2(&self, #[zbus(header)] header: Header<'_>) -> Vec<(String, String)> {
        if !self.authorized(&header, "profiles2").await {
            return Vec::new();
        }
        self.daemon.profiles2().await
    }

    #[zbus(name = "profile_info")]
    async fn profile_info(
        &self,
        profile_name: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> (bool, String, String, String) {
        if !self.authorized(&header, "profile_info").await {
            return (false, String::new(), String::new(), "Unauthorized".to_string());
        }
        self.daemon.profile_info(profile_name).await
    }

    #[zbus(name = "recommend_profile")]
    async fn recommend_profile(&self, #[zbus(header)] header: Header<'_>) -> String {
        if !self.authorized(&header, "recommend_profile").await {
            return String::new();
        }
        self.daemon.recommend_profile().await
    }

    #[zbus(name = "verify_profile")]
    async fn verify_profile(&self, #[zbus(header)] header: Header<'_>) -> bool {
        if !self.authorized(&header, "verify_profile").await {
            return false;
        }
        !self.daemon.active_profile().await.is_empty()
    }

    #[zbus(name = "verify_profile_ignore_missing")]
    async fn verify_profile_ignore_missing(&self, #[zbus(header)] header: Header<'_>) -> bool {
        self.verify_profile(header).await
    }

    #[zbus(name = "log_capture_start")]
    async fn log_capture_start(
        &self,
        _log_level: i32,
        _timeout: i32,
        #[zbus(header)] header: Header<'_>,
    ) -> String {
        if !self.authorized(&header, "log_capture_start").await {
            return String::new();
        }
        String::new()
    }

    #[zbus(name = "log_capture_finish")]
    async fn log_capture_finish(&self, _token: &str, #[zbus(header)] header: Header<'_>) -> String {
        if !self.authorized(&header, "log_capture_finish").await {
            return String::new();
        }
        String::new()
    }

    #[zbus(name = "get_all_plugins")]
    async fn get_all_plugins(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> HashMap<String, HashMap<String, String>> {
        if !self.authorized(&header, "get_all_plugins").await {
            return HashMap::new();
        }
        HashMap::from([
            (
                "cpu".to_string(),
                HashMap::from([
                    ("governor".to_string(), String::new()),
                    (
                        "energy_performance_preference".to_string(),
                        String::new(),
                    ),
                ]),
            ),
            (
                "sysctl".to_string(),
                HashMap::from([("".to_string(), String::new())]),
            ),
            (
                "vm".to_string(),
                HashMap::from([("dirty_bytes".to_string(), String::new())]),
            ),
            (
                "disk".to_string(),
                HashMap::from([
                    ("readahead".to_string(), String::new()),
                    ("elevator".to_string(), String::new()),
                ]),
            ),
            (
                "acpi".to_string(),
                HashMap::from([("platform_profile".to_string(), String::new())]),
            ),
        ])
    }

    #[zbus(name = "get_plugin_documentation")]
    async fn get_plugin_documentation(
        &self,
        _plugin_name: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> String {
        if !self.authorized(&header, "get_plugin_documentation").await {
            return String::new();
        }
        String::new()
    }

    #[zbus(name = "get_plugin_hints")]
    async fn get_plugin_hints(
        &self,
        _plugin_name: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> HashMap<String, String> {
        if !self.authorized(&header, "get_plugin_hints").await {
            return HashMap::new();
        }
        HashMap::new()
    }

    #[zbus(name = "register_socket_signal_path")]
    async fn register_socket_signal_path(
        &self,
        _path: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> bool {
        if !self.authorized(&header, "register_socket_signal_path").await {
            return false;
        }
        false
    }

    #[zbus(name = "instance_acquire_devices")]
    async fn instance_acquire_devices(
        &self,
        _devices: &str,
        _instance_name: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> (bool, String) {
        if !self.authorized(&header, "instance_acquire_devices").await {
            return polkit::unauthorized_pair();
        }
        (false, "Not supported".to_string())
    }

    #[zbus(name = "get_instances")]
    async fn get_instances(
        &self,
        _plugin_name: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> (bool, String, Vec<(String, String)>) {
        if !self.authorized(&header, "get_instances").await {
            return (false, "Unauthorized".to_string(), Vec::new());
        }
        (true, "OK".to_string(), Vec::new())
    }

    #[zbus(name = "instance_get_devices")]
    async fn instance_get_devices(
        &self,
        _instance_name: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> (bool, String, Vec<String>) {
        if !self.authorized(&header, "instance_get_devices").await {
            return (false, "Unauthorized".to_string(), Vec::new());
        }
        (false, "Not supported".to_string(), Vec::new())
    }

    #[zbus(name = "instance_create")]
    async fn instance_create(
        &self,
        _plugin_name: &str,
        _instance_name: &str,
        _options: HashMap<String, String>,
        #[zbus(header)] header: Header<'_>,
    ) -> (bool, String) {
        if !self.authorized(&header, "instance_create").await {
            return polkit::unauthorized_pair();
        }
        (false, "Not supported".to_string())
    }

    #[zbus(name = "instance_destroy")]
    async fn instance_destroy(
        &self,
        _instance_name: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> (bool, String) {
        if !self.authorized(&header, "instance_destroy").await {
            return polkit::unauthorized_pair();
        }
        (false, "Not supported".to_string())
    }
}

pub async fn spawn_server(daemon: Arc<Daemon>) -> zbus::Result<Connection> {
    info!(
        "Registering TuneD-compatible D-Bus service {} on {}",
        config::NAMESPACE,
        config::DBUS_OBJECT
    );

    let polkit = Polkit::new(Connection::system().await?)
        .await
        .map_err(|error| zbus::Error::Failure(error.to_string()))?;

    let conn = Builder::system()?
        .name(config::NAMESPACE)?
        .serve_at(
            config::DBUS_OBJECT,
            TunedController { daemon, polkit },
        )?
        .build()
        .await?;

    Ok(conn)
}
