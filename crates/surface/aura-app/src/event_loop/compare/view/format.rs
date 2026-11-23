//! How amounts, durations, token counts, headlines, arms and statuses
//! read wherever `/compare` shows them.

use aura_core_types::{ArmSpec, CompareStatus};
use aura_gate::compare::NO_DELEGATE;
use aura_model::model_catalog::route_label;
use aura_ui_terminal::events::BenchCost;
use std::borrow::Cow;

/// `$0.004`, `$1.25`, or `free` for nothing.
pub(crate) fn money(usd: f64) -> String {
    if usd <= 0.0 {
        "free".to_string()
    } else if usd < 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.2}")
    }
}

/// `usd` as [`money`] when every call in it had a price, `$0.11+?` when
/// `unpriced_calls` of them had none, `unknown` when none of the priced
/// ones cost anything.
pub(crate) fn cost(usd: f64, unpriced_calls: u32) -> String {
    BenchCost::new(usd, unpriced_calls).read(money)
}

/// `850ms`, `42s`, `3m 05s`.
pub(crate) fn duration(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    let secs = ms / 1_000;
    if secs < 60 {
        return format!("{secs}s");
    }
    format!("{}m {:02}s", secs / 60, secs % 60)
}

/// `950`, `12k`, `1.4M` tokens.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{}k", (n as f64 / 1_000.0).round())
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// The first line of `text` that isn't blank, trimmed and cut to `max`
/// characters, the last of them `…` when it is cut.
pub(crate) fn headline(text: &str, max: usize) -> String {
    let line = (text.lines())
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().nth(max).is_none() {
        return line.to_string();
    }
    let cut = (line.char_indices())
        .nth(max.saturating_sub(1))
        .map_or(0, |(at, _)| at);
    format!("{}…", &line[..cut])
}

/// `s` unless `n` is one, for the noun that counts it.
pub(crate) fn plural<N: From<u8> + PartialEq>(n: N) -> &'static str {
    if n == N::from(1) {
        ""
    } else {
        "s"
    }
}

/// How an arm reads: its model and route, or Gate and whether its lead
/// must delegate.
pub(crate) fn arm_label(spec: &ArmSpec) -> String {
    match spec {
        ArmSpec::Gate { delegate: true } => "gate".to_string(),
        ArmSpec::Gate { delegate: false } => {
            format!("gate without enforced delegation ({NO_DELEGATE})")
        }
        ArmSpec::Model { model } => route_label(model),
        ArmSpec::ByDifficulty { easy, medium, hard } => format!(
            "by difficulty: easy → {} · medium → {} · hard → {}",
            route_label(easy),
            route_label(medium),
            route_label(hard)
        ),
    }
}

/// How `status` reads: `done`, `stopped`, `applied a-2`.
pub(super) fn status(status: &CompareStatus) -> Cow<'static, str> {
    match status {
        CompareStatus::Running => "running".into(),
        CompareStatus::Done => "done".into(),
        CompareStatus::Cancelled => "stopped".into(),
        CompareStatus::Applied(run) => format!("applied {run}").into(),
        CompareStatus::Dismissed => "dismissed".into(),
    }
}
