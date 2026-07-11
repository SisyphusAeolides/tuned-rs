pub mod config;
pub mod daemon;
pub mod engine;
pub mod hal;
pub mod dynamic;
pub mod ipc;
pub mod telemetry;
pub mod monitor;
pub mod polkit;
pub mod profile;
pub mod rollback;
pub mod tuning;

pub mod ppd;

#[derive(Debug)]
pub enum DaemonEvent {
    Hardware { action: String, device: String },
}
