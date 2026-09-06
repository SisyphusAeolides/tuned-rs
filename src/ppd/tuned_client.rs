use anyhow::{Context, Result};
use zbus::proxy;

#[proxy(
    interface = "com.redhat.tuned.control",
    default_service = "com.redhat.tuned",
    default_path = "/Tuned"
)]
trait Tuned {
    #[zbus(name = "active_profile")]
    fn active_profile(&self) -> zbus::Result<String>;

    #[zbus(name = "switch_profile")]
    fn switch_profile(&self, profile_name: &str) -> zbus::Result<(bool, String)>;

    #[zbus(name = "profiles")]
    fn profiles(&self) -> zbus::Result<Vec<String>>;
}

pub struct TunedClient {
    proxy: TunedProxy<'static>,
}

impl TunedClient {
    pub async fn connect(connection: &zbus::Connection) -> Result<Self> {
        let proxy = TunedProxy::new(connection)
            .await
            .context("TuneD not found on the D-Bus; ensure tuned-rs is running")?;
        Ok(Self { proxy })
    }

    pub async fn active_profile(&self) -> Result<String> {
        Ok(self.proxy.active_profile().await?)
    }

    pub async fn profiles(&self) -> Result<Vec<String>> {
        Ok(self.proxy.profiles().await?)
    }

    pub async fn switch_profile(&self, profile_name: &str) -> Result<(bool, String)> {
        self.proxy
            .switch_profile(profile_name)
            .await
            .with_context(|| format!("Failed to switch TuneD profile to '{profile_name}'"))
    }
}

#[derive(Debug, Clone)]
pub struct ProfileChangedEvent {
    pub profile_name: String,
    pub result: bool,
    pub errstr: String,
}
