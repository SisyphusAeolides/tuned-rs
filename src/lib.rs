pub mod config;
pub mod daemon;
pub mod dynamic;
pub mod engine;
pub mod hal;
pub mod instances;
pub mod ipc;
pub mod log_capture;
pub mod monitor;
pub mod plugins;
pub mod polkit;
pub mod profile;
pub mod profile_runtime;
pub mod profile_units;
pub mod rollback;
pub mod socket_signals;
pub mod telemetry;
pub mod tuning;
pub mod verification;

pub mod ppd;

#[derive(Debug)]
pub enum DaemonEvent {
    Hardware { action: String, device: String },
}
