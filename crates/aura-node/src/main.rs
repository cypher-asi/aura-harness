//! Aura Node binary entry point.

use aura_node::{Node, NodeConfig};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env().add_directive("aura=info".parse()?))
        .init();

    // Load config from environment
    let config = NodeConfig::from_env();

    // Run the node
    Node::new(config).run().await
}
