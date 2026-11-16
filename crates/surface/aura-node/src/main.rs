use tracing_subscriber::{fmt, prelude::*, EnvFilter};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::registry()
        .with(fmt::layer().event_format(aura_node::console_format::AuraConsoleFormat::new()))
        .with(EnvFilter::from_default_env().add_directive("aura=info".parse()?))
        .init();
    aura_node::run_node().await
}
