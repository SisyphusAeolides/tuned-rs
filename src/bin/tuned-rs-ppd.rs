use anyhow::{Context, Result};
use tracing::info;
use tracing_subscriber::EnvFilter;
use tuned_rs::ppd;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set tracing subscriber");

    info!("Starting tuned-rs-ppd daemon...");
    ppd::run().await.context("tuned-rs-ppd failed")
}
