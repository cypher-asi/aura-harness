//! Aura Node binary entry point.

use aura_runtime::{Node, NodeConfig};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env().add_directive("aura=info".parse()?))
        .init();

    let config = NodeConfig::from_env();

    // Run the node
    Node::new(config).run().await
}
