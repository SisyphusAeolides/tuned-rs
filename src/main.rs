use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::EnvFilter;
use tuned_rs::{
    config, daemon, ipc, log_capture, monitor, profile, rollback, unix_socket, DaemonEvent,
};

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .with(log_capture::CaptureLayer::new(log_capture::global_store()));
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");

    info!("Starting tuned-rs daemon...");

    settle_udev().await;

    let profile_dirs: Vec<_> = config::profile_dirs_from_env()
        .into_iter()
        .map(config::resolve_path_buf)
        .collect();
    let catalog = profile::ProfileCatalog::load_from_dirs(&profile_dirs)
        .context("Failed to load TuneD profiles")?;
    let rollback = std::sync::Arc::new(rollback::Rollback::load()?);
    let daemon = daemon::Daemon::new(catalog, rollback);

    if !config::daemon_enabled() {
        if !daemon.start().await? {
            anyhow::bail!("One-shot TuneD profile application failed");
        }
        info!("One-shot TuneD profile application completed");
        return Ok(());
    }

    let (tx, mut rx) = mpsc::channel::<DaemonEvent>(32);
    monitor::spawn_power_monitor(tx)?;
    let _dbus_conn = if config::dbus_enabled() {
        Some(ipc::spawn_server(daemon.clone()).await?)
    } else {
        None
    };
    let _unix_socket = unix_socket::spawn(daemon.clone())?;

    if !daemon.start().await? {
        warn!("Daemon started without applying a profile");
    }

    info!("All modules online. Entering main event loop...");

    let (adaptive_stop_tx, adaptive_stop_rx) = watch::channel(false);
    let adaptive_task = if config::chaos_auto_profile() {
        Some(tokio::spawn(run_adaptive_monitor(
            Arc::clone(&daemon),
            adaptive_stop_rx,
        )))
    } else {
        None
    };

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

    let _ = adaptive_stop_tx.send(true);
    if let Some(task) = adaptive_task {
        let _ = task.await;
    }

    daemon.stop(config::rollback_on_exit()).await;
    info!("Shutting down tuned-rs...");
    Ok(())
}

async fn run_adaptive_monitor(daemon: Arc<daemon::Daemon>, mut stop: watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(config::chaos_control_interval());
    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(error) = daemon.adaptive_profile_step().await {
                    warn!("Chaos-assisted adaptive tuning step failed: {error}");
                }
            }
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    break;
                }
            }
        }
    }
}

async fn settle_udev() {
    let timeout = config::startup_udev_settle_wait();
    if timeout == 0 || std::env::var_os("TUNED_RS_ROOT").is_some() {
        return;
    }
    info!("Waiting up to {timeout} second(s) for udev to settle");
    match tokio::process::Command::new("udevadm")
        .args(["settle", "--timeout", &timeout.to_string()])
        .status()
        .await
    {
        Ok(status) if status.success() => info!("udev settled"),
        Ok(status) => warn!("udevadm settle exited with {status}"),
        Err(error) => warn!("Failed to wait for udev: {error}"),
    }
}
