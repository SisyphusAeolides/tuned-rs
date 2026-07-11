use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use tuned_rs::{config, daemon, ipc, monitor, profile, rollback, DaemonEvent};

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set tracing subscriber");

    info!("Starting tuned-rs daemon...");

    let profile_dirs: Vec<_> = config::profile_dirs_from_env()
        .into_iter()
        .map(config::resolve_path_buf)
        .collect();
    let catalog = profile::ProfileCatalog::load_from_dirs(&profile_dirs)
        .context("Failed to load TuneD profiles")?;
    let rollback = std::sync::Arc::new(rollback::Rollback::load()?);
    let daemon = daemon::Daemon::new(catalog, rollback);

    let (tx, mut rx) = mpsc::channel::<DaemonEvent>(32);
    monitor::spawn_power_monitor(tx)?;
    let _dbus_conn = ipc::spawn_server(daemon.clone()).await?;

    if !daemon.start().await? {
        warn!("Daemon started without applying a profile");
    }

    info!("All modules online. Entering main event loop...");

    loop {
        tokio::select! {
            event = rx.recv() => {
                let Some(event) = event else {
                    break;
                };

                let DaemonEvent::Hardware { action, device } = event;
                info!("Main Loop: Hardware shift - [{action}] on {device}");
                if let Err(error) = daemon.reapply_active_profile().await {
                    error!("Failed to reapply active profile after hardware event: {error}");
                }
            }
            result = tokio::signal::ctrl_c() => {
                match result {
                    Ok(()) => info!("Received shutdown signal"),
                    Err(error) => error!("Failed to listen for shutdown signal: {error}"),
                }
                break;
            }
        }
    }

    daemon.stop(config::rollback_on_exit()).await;
    info!("Shutting down tuned-rs...");
    Ok(())
}
