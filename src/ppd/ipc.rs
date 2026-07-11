use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{error, info};
use zbus::fdo;
use zbus::message::Header;
use zbus::zvariant::Value;
use zbus::{connection::Builder, interface, Connection, SignalContext};

use crate::ppd::config::{self, API_VERSION};
use crate::ppd::controller::PpdCore;
use crate::ppd::polkit::{self, Polkit};

pub struct UPowerProfiles {
    core: Arc<PpdCore>,
    polkit: Polkit,
}

pub struct HadessProfiles {
    core: Arc<PpdCore>,
    polkit: Polkit,
}

struct PropertyEmitter {
    conn: Connection,
}

impl PropertyEmitter {
    async fn properties_changed(
        &self,
        interface: &str,
        path: &str,
        changes: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<()> {
        self.conn
            .emit_signal(
                None::<&str>,
                path,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &(interface, changes, Vec::<String>::new()),
            )
            .await?;
        Ok(())
    }

    async fn active_profile_changed(&self, value: &str) -> zbus::Result<()> {
        self.properties_changed(
            config::UPOWER_BUS,
            config::UPOWER_PATH,
            HashMap::from([("ActiveProfile", Value::from(value))]),
        )
        .await?;
        self.properties_changed(
            config::HADESS_BUS,
            config::HADESS_PATH,
            HashMap::from([("ActiveProfile", Value::from(value))]),
        )
        .await
    }

    async fn performance_degraded_changed(&self, value: &str) -> zbus::Result<()> {
        self.properties_changed(
            config::UPOWER_BUS,
            config::UPOWER_PATH,
            HashMap::from([("PerformanceDegraded", Value::from(value))]),
        )
        .await?;
        self.properties_changed(
            config::HADESS_BUS,
            config::HADESS_PATH,
            HashMap::from([("PerformanceDegraded", Value::from(value))]),
        )
        .await
    }

    async fn active_profile_holds_changed(
        &self,
        holds: &[HashMap<String, String>],
    ) -> zbus::Result<()> {
        let dbus_holds: Vec<HashMap<String, Value<'_>>> = holds
            .iter()
            .map(|hold| {
                hold.iter()
                    .map(|(key, value)| (key.clone(), Value::from(value.as_str())))
                    .collect()
            })
            .collect();
        let value = Value::from(dbus_holds);
        self.properties_changed(
            config::UPOWER_BUS,
            config::UPOWER_PATH,
            HashMap::from([("ActiveProfileHolds", value.try_clone()?)]),
        )
        .await?;
        self.properties_changed(
            config::HADESS_BUS,
            config::HADESS_PATH,
            HashMap::from([("ActiveProfileHolds", value)]),
        )
        .await
    }

    async fn profile_released(&self, cookie: u32) -> zbus::Result<()> {
        UPowerProfiles::profile_released(
            &SignalContext::new(&self.conn, config::UPOWER_PATH)?,
            cookie,
        )
        .await?;
        HadessProfiles::profile_released(
            &SignalContext::new(&self.conn, config::HADESS_PATH)?,
            cookie,
        )
        .await
    }
}

macro_rules! ppd_interface {
    ($iface:ident, $namespace:literal) => {
        #[interface(name = $namespace)]
        impl $iface {
            #[zbus(property)]
            async fn active_profile(&self) -> String {
                self.core.active_profile().await
            }

            #[zbus(property)]
            async fn set_active_profile(&self, value: &str) -> zbus::Result<()> {
                self.core
                    .set_active_profile(value)
                    .await
                    .map_err(|error| zbus::Error::Failure(error.to_string()))
            }

            #[zbus(property)]
            async fn profiles(&self) -> Vec<HashMap<String, Value<'_>>> {
                self.core
                    .profiles()
                    .await
                    .into_iter()
                    .map(|profile| {
                        profile
                            .into_iter()
                            .map(|(key, value)| (key, Value::from(value)))
                            .collect()
                    })
                    .collect()
            }

            #[zbus(property)]
            async fn actions(&self) -> Vec<String> {
                Vec::new()
            }

            #[zbus(property)]
            async fn performance_degraded(&self) -> String {
                self.core.performance_degraded().await.as_str().to_string()
            }

            #[zbus(property)]
            async fn active_profile_holds(&self) -> Vec<HashMap<String, String>> {
                self.core.active_profile_holds().await
            }

            #[zbus(property)]
            async fn version(&self) -> String {
                API_VERSION.to_string()
            }

            #[zbus(name = "HoldProfile")]
            async fn hold_profile(
                &self,
                profile: &str,
                reason: &str,
                app_id: &str,
                #[zbus(header)] header: Header<'_>,
            ) -> fdo::Result<u32> {
                if !self
                    .polkit
                    .authorize(polkit::sender_from_header(&header).as_deref(), "hold-profile")
                    .await
                {
                    return Err(fdo::Error::AccessDenied(
                        "Unauthorized".to_string(),
                    ));
                }
                let caller = header
                    .sender()
                    .ok_or_else(|| fdo::Error::Failed("Missing caller".into()))?
                    .to_string();
                self.core
                    .add_hold(profile, reason, app_id, &caller)
                    .await
                    .map_err(|error| fdo::Error::Failed(error.to_string()))
            }

            #[zbus(name = "ReleaseProfile")]
            async fn release_profile(
                &self,
                cookie: u32,
                #[zbus(header)] header: Header<'_>,
            ) -> fdo::Result<()> {
                if !self
                    .polkit
                    .authorize(
                        polkit::sender_from_header(&header).as_deref(),
                        "release-profile",
                    )
                    .await
                {
                    return Err(fdo::Error::AccessDenied(
                        "Unauthorized".to_string(),
                    ));
                }
                let caller = header
                    .sender()
                    .ok_or_else(|| fdo::Error::Failed("Missing caller".into()))?
                    .to_string();
                self.core
                    .release_hold(cookie, &caller)
                    .await
                    .map_err(|error| fdo::Error::Failed(error.to_string()))
            }

            #[zbus(signal, name = "ProfileReleased")]
            async fn profile_released(ctxt: &SignalContext<'_>, cookie: u32) -> zbus::Result<()>;
        }
    };
}

ppd_interface!(UPowerProfiles, "org.freedesktop.UPower.PowerProfiles");
ppd_interface!(HadessProfiles, "net.hadess.PowerProfiles");

pub async fn spawn_servers(
    core: Arc<PpdCore>,
    mut hold_release_rx: mpsc::Receiver<u32>,
) -> Result<Connection> {
    info!("Registering Power Profiles D-Bus services");

    let system = Connection::system().await?;
    let upower_polkit = Polkit::new(system.clone(), config::UPOWER_BUS);
    let hadess_polkit = Polkit::new(system.clone(), config::HADESS_BUS);

    let conn = Builder::system()?
        .name(config::UPOWER_BUS)?
        .name(config::HADESS_BUS)?
        .serve_at(
            config::UPOWER_PATH,
            UPowerProfiles {
                core: core.clone(),
                polkit: upower_polkit,
            },
        )?
        .serve_at(
            config::HADESS_PATH,
            HadessProfiles {
                core: core.clone(),
                polkit: hadess_polkit,
            },
        )?
        .build()
        .await?;

    let emitter = Arc::new(PropertyEmitter {
        conn: conn.clone(),
    });

    let emitter_for_releases = emitter.clone();
    tokio::spawn(async move {
        while let Some(cookie) = hold_release_rx.recv().await {
            if let Err(error) = emitter_for_releases.profile_released(cookie).await {
                error!("Failed to emit ProfileReleased for cookie {cookie}: {error}");
            }
        }
    });

    let emitter_for_sync = emitter.clone();
    tokio::spawn(async move {
        spawn_property_sync(core, emitter_for_sync).await;
    });

    Ok(conn)
}

async fn spawn_property_sync(core: Arc<PpdCore>, emitter: Arc<PropertyEmitter>) {
    let mut last_active = core.active_profile().await;
    let mut last_degraded = core.performance_degraded().await.as_str().to_string();
    let mut last_holds = core.active_profile_holds().await;

    let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        interval.tick().await;
        let active = core.active_profile().await;
        if active != last_active {
            if let Err(error) = emitter.active_profile_changed(&active).await {
                error!("Failed to emit ActiveProfile change: {error}");
            }
            last_active = active;
        }

        let degraded = core.performance_degraded().await.as_str().to_string();
        if degraded != last_degraded {
            if let Err(error) = emitter.performance_degraded_changed(&degraded).await {
                error!("Failed to emit PerformanceDegraded change: {error}");
            }
            last_degraded = degraded;
        }

        let holds = core.active_profile_holds().await;
        if holds != last_holds {
            if let Err(error) = emitter.active_profile_holds_changed(&holds).await {
                error!("Failed to emit ActiveProfileHolds change: {error}");
            }
            last_holds = holds;
        }
    }
}
