pub mod config;
pub mod controller;
pub mod ipc;
pub mod polkit;
pub mod tuned_client;

use anyhow::Result;

pub async fn run() -> Result<()> {
    controller::run().await
}
