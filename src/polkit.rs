use anyhow::{Context, Result};
use tracing::{debug, error, warn};
use zbus::message::Header;
use zbus::{Connection, Proxy};
use zbus::zvariant::Value;

use crate::config;

const POLKIT_BUS: &str = "org.freedesktop.PolicyKit1";
const POLKIT_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const POLKIT_INTERFACE: &str = "org.freedesktop.PolicyKit1.Authority";

pub struct Polkit {
    connection: Connection,
}

impl Polkit {
    pub async fn new(connection: Connection) -> Result<Self> {
        Ok(Self { connection })
    }

    pub async fn authorize(&self, sender: Option<&str>, action: &str) -> bool {
        let Some(sender) = sender else {
            return true;
        };

        let action_id = format!("{}.{action}", config::NAMESPACE);
        debug!("Checking polkit authorization for '{action_id}' from '{sender}'");

        match self.check_authorization(sender, &action_id).await {
            Ok(true) => true,
            Ok(false) => {
                warn!("Caller '{sender}' is not authorized for '{action_id}'");
                false
            }
            Err(error) => self.fallback_root_authorization(sender, &action_id, error).await,
        }
    }

    async fn check_authorization(&self, sender: &str, action_id: &str) -> Result<bool> {
        let proxy = Proxy::new(
            &self.connection,
            POLKIT_BUS,
            POLKIT_PATH,
            POLKIT_INTERFACE,
        )
        .await?;

        let subject_details =
            std::collections::HashMap::from([("name", Value::from(sender))]);
        let subject = ("system-bus-name", subject_details);
        let details: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        let (authorized, _, _): (bool, bool, std::collections::HashMap<String, String>) = proxy
            .call(
                "CheckAuthorization",
                &(subject, action_id, details, 1u32, ""),
            )
            .await
            .context("PolicyKit CheckAuthorization failed")?;

        Ok(authorized)
    }

    async fn fallback_root_authorization(
        &self,
        sender: &str,
        action_id: &str,
        error: anyhow::Error,
    ) -> bool {
        warn!("PolicyKit error for '{action_id}': {error}");
        match get_connection_unix_user(&self.connection, sender).await {
            Ok(0) => {
                warn!(
                    "PolicyKit unavailable; allowing root fallback for caller '{sender}'"
                );
                true
            }
            Ok(_) => {
                warn!(
                    "PolicyKit unavailable; denying non-root caller '{sender}' for '{action_id}'"
                );
                false
            }
            Err(fallback_error) => {
                error!(
                    "PolicyKit and fallback authorization failed for '{sender}': {fallback_error}"
                );
                false
            }
        }
    }
}

async fn get_connection_unix_user(connection: &Connection, name: &str) -> Result<u32> {
    let proxy = Proxy::new(
        connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await?;
    let uid: u32 = proxy.call("GetConnectionUnixUser", &(name,)).await?;
    Ok(uid)
}

pub fn unauthorized_pair() -> (bool, String) {
    (false, "Unauthorized".to_string())
}

pub fn sender_from_header(header: &Header<'_>) -> Option<String> {
    header.sender().map(|name| name.to_string())
}
