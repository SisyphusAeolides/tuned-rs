use std::thread;
use std::time::Duration;
use tokio_udev::MonitorBuilder;
use tracing::{error, info};
use tokio::sync::mpsc;
use crate::DaemonEvent;

/// Spawns a dedicated thread for libudev because `MonitorSocket` is `!Send`.
pub fn spawn_power_monitor(tx: mpsc::Sender<DaemonEvent>) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("tuned-rs-udev".into())
        .spawn(move || {
            let mut monitor = match MonitorBuilder::new()
                .and_then(|builder| builder.match_subsystem("power_supply"))
                .and_then(|builder| builder.listen())
            {
                Ok(monitor) => monitor,
                Err(e) => {
                    error!("Failed to start udev power_supply monitor: {e}");
                    return;
                }
            };

            info!("udev monitor listening for ACPI/power_supply events...");

            loop {
                match monitor.next() {
                    Some(event) => {
                        let action = event
                            .action()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        let device = event.sysname().to_string_lossy().into_owned();

                        info!("Hardware Event: [{action}] on {device}");

                        if tx
                            .blocking_send(DaemonEvent::Hardware { action, device })
                            .is_err()
                        {
                            info!("Event channel closed; stopping udev monitor thread");
                            return;
                        }
                    }
                    None => thread::sleep(Duration::from_millis(250)),
                }
            }
        })?;

    Ok(())
}
