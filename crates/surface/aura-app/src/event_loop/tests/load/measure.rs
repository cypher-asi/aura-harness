//! Statistics the load harness prints: percentiles of durations, by
//! kind, as a table.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

/// Percentiles of a set of durations.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Spread {
    pub(super) count: usize,
    pub(super) total: Duration,
    pub(super) p50: Duration,
    pub(super) p95: Duration,
    pub(super) p99: Duration,
    pub(super) max: Duration,
}

impl Spread {
    pub(super) fn of(samples: &[Duration]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        // The sample at `permille` thousandths of the way through.
        let at = |permille: usize| sorted[(sorted.len() - 1) * permille / 1000];
        Self {
            count: sorted.len(),
            total: sorted.iter().sum(),
            p50: at(500),
            p95: at(950),
            p99: at(990),
            max: sorted[sorted.len() - 1],
        }
    }
}

/// Durations by kind.
#[derive(Default)]
pub(super) struct ByKind(pub(super) BTreeMap<&'static str, Vec<Duration>>);

impl ByKind {
    pub(super) fn add(&mut self, kind: &'static str, took: Duration) {
        self.0.entry(kind).or_default().push(took);
    }

    /// The kinds by total time, largest first.
    pub(super) fn spreads(&self) -> Vec<(&'static str, Spread)> {
        let mut rows: Vec<(&'static str, Spread)> = self
            .0
            .iter()
            .map(|(kind, samples)| (*kind, Spread::of(samples)))
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.1.total));
        rows
    }
}

fn ms(d: Duration) -> String {
    format!("{:.2}", d.as_secs_f64() * 1000.0)
}

/// One spread as a table row named `label`.
pub(super) fn row(out: &mut String, label: &str, spread: &Spread) {
    let _ = writeln!(
        out,
        "  {label:<28} {:>8} {:>10} {:>8} {:>8} {:>8} {:>9}",
        spread.count,
        ms(spread.total),
        ms(spread.p50),
        ms(spread.p95),
        ms(spread.p99),
        ms(spread.max),
    );
}

/// The header of a table of spreads, in milliseconds.
pub(super) fn header(out: &mut String, title: &str) {
    let _ = writeln!(out, "{title}");
    let _ = writeln!(
        out,
        "  {:<28} {:>8} {:>10} {:>8} {:>8} {:>8} {:>9}",
        "", "count", "total ms", "p50", "p95", "p99", "max"
    );
}

/// The resident memory of this process, in MiB, where `/proc` says it.
pub(super) fn rss_mib() -> Option<f64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: f64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096.0 / (1024.0 * 1024.0))
}
