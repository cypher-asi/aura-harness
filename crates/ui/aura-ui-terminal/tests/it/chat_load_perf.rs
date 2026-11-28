//! Where the time goes when a chat is loaded: applying the history the
//! session sends, laying out its first frame, the frames after it, and
//! switching between two chats already held. Ignored by default; run with
//!
//! ```text
//! cargo test --release -p aura-ui-terminal --test it chat_load_perf:: -- --ignored --nocapture
//! ```
//!
//! `AURA_PERF_ENTRIES` lists the chat sizes to try (default `300,1000`).
//! The session sends at most `HISTORY_MESSAGE_LIMIT` (300) messages, so
//! that is the realistic load and 1000 the worst a caller could send.
//! Everything here goes through the public API, so it measures the same
//! paths the app does.

mod app_switch;
mod bench;
mod chat_load;
mod code_blocks;
mod fixtures;
mod markdown;
mod report;

use app_switch::app_switch_bench;
use chat_load::{breakdown, main_bench};
use code_blocks::code_breakdown;

fn env_list(name: &str, default: &[usize]) -> Vec<usize> {
    std::env::var(name).ok().map_or_else(
        || default.to_vec(),
        |v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect(),
    )
}

#[test]
#[ignore = "load test; run with --ignored --nocapture"]
fn chat_load_perf() {
    // The backend sends at most 300 messages (`HISTORY_MESSAGE_LIMIT`), so
    // that is the realistic load; 1000 is the worst a caller could send.
    for entries in env_list("AURA_PERF_ENTRIES", &[300, 1000]) {
        main_bench(entries);
        app_switch_bench(entries);
        breakdown(entries);
    }
    code_breakdown(300);
}

#[test]
#[ignore = "load test; run with --ignored --nocapture"]
fn app_switch_perf() {
    for entries in env_list("AURA_PERF_ENTRIES", &[300, 1000]) {
        app_switch_bench(entries);
    }
}
