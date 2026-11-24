//! Live checks against the real boxd API. Ignored by default; run with
//!
//! ```sh
//! cargo test -p aura-engine-cloud --test live_boxd -- --ignored --nocapture
//! ```
//!
//! with `BOXD_API_KEY` set (a `.env` works). `read_only_calls_succeed`
//! creates nothing and costs nothing.

use aura_engine_cloud::{BoxdProvider, VmProvider};

fn provider() -> BoxdProvider {
    let _ = aura_config::env_file::load();
    let config = aura_config::CloudConfig::from_env().expect("cloud settings");
    assert!(config.is_configured(), "{}", aura_config::MISSING_CREDENTIAL_NOTICE);
    BoxdProvider::from_config(&config).expect("provider")
}

#[tokio::test]
#[ignore = "talks to the real boxd API; needs BOXD_API_KEY"]
async fn read_only_calls_succeed() {
    let boxd = provider();
    let limit = boxd.machine_limit().await.expect("machine limit");
    println!("machines: {} of {}", limit.count, limit.max);
    let machines = boxd.list(&[]).await.expect("list machines");
    println!("listed {} machines", machines.len());
    for machine in &machines {
        println!("  {} {} {}", machine.name, machine.status.as_str(), machine.region);
    }
    match boxd.balance().await {
        Ok(balance) => println!("balance: {balance:?}"),
        Err(err) => println!("balance unavailable (fine for API keys): {err}"),
    }
    let snapshot = boxd.snapshot("aura-base").await.expect("snapshot lookup");
    println!("aura-base snapshot: {snapshot:?}");
}
