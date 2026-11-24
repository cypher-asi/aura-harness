//! What a machine costs: boxd's meter reading, plus a live estimate for
//! the time since that reading.
//!
//! boxd meters every second but settles every 15 minutes and lags up to
//! about 25 minutes, so its figure trails. [`CostModel::estimate`] prices
//! the host samples taken after the reading's end with the same rules
//! boxd bills by: active vCPU only, memory in use, disk written.
//!
//! ## Determinism
//!
//! The total is a pure function of the latest `usage` ledger row and the
//! samples: replaying the same inputs gives the same figure.

use aura_config::CloudConfig;
use aura_core_types::{PriceRates, VmEvent, VmPayload, VmUsage};
use serde::{Deserialize, Serialize};

use crate::provider::UsageReading;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const MS_PER_HOUR: f64 = 3_600_000.0;

/// One host sample, as far as cost is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CostSample {
    /// When it was taken, Unix milliseconds.
    pub at_ms: u64,
    /// CPU-seconds busy since boot, summed over CPUs.
    pub cpu_busy_secs: f64,
    /// Memory in use, bytes.
    pub mem_used_bytes: u64,
    /// Disk written, bytes.
    pub disk_used_bytes: u64,
}

/// A machine's cost as shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostEstimate {
    /// boxd's figure, through `metered_until_ms`.
    pub metered: f64,
    /// End of boxd's figure, Unix milliseconds (0 = none yet).
    pub metered_until_ms: u64,
    /// Whether boxd's figure is final.
    pub complete: bool,
    /// Estimated cost since `metered_until_ms`.
    pub estimated: f64,
    /// Estimated rate right now, per hour.
    pub per_hour: f64,
    /// Currency of every figure.
    pub currency: String,
}

impl CostEstimate {
    /// Metered plus estimated.
    #[must_use]
    pub fn total(&self) -> f64 {
        self.metered + self.estimated
    }
}

/// Prices usage.
#[derive(Debug, Clone, PartialEq)]
pub struct CostModel {
    rates: PriceRates,
}

impl CostModel {
    /// A model at `rates`.
    #[must_use]
    pub const fn new(rates: PriceRates) -> Self {
        Self { rates }
    }

    /// boxd's list prices with `config`'s overrides.
    #[must_use]
    pub fn from_config(config: &CloudConfig) -> Self {
        let mut rates = PriceRates::boxd_list();
        if let Some(rate) = config.price_vcpu_hour {
            rates.vcpu_hour = rate;
        }
        if let Some(rate) = config.price_mem_gib_hour {
            rates.mem_gib_hour = rate;
        }
        if let Some(rate) = config.price_disk_gib_hour {
            rates.disk_gib_hour = rate;
        }
        Self { rates }
    }

    /// The rates in force.
    #[must_use]
    pub const fn rates(&self) -> &PriceRates {
        &self.rates
    }

    /// The cost of the stretch between two samples.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn between(&self, from: &CostSample, to: &CostSample) -> f64 {
        if to.at_ms <= from.at_ms {
            return 0.0;
        }
        let hours = (to.at_ms - from.at_ms) as f64 / MS_PER_HOUR;
        // A reboot resets the counter; count nothing for that stretch.
        let vcpu_hours = (to.cpu_busy_secs - from.cpu_busy_secs).max(0.0) / 3600.0;
        let mem_gib = (from.mem_used_bytes as f64 + to.mem_used_bytes as f64) / 2.0 / GIB;
        let disk_gib = (from.disk_used_bytes as f64 + to.disk_used_bytes as f64) / 2.0 / GIB;
        self.rates
            .price(vcpu_hours, mem_gib * hours, disk_gib * hours)
    }

    /// boxd's reading plus an estimate over `samples` taken after it.
    /// Samples must be in time order.
    #[must_use]
    pub fn estimate(&self, metered: Option<&UsageReading>, samples: &[CostSample]) -> CostEstimate {
        let (metered_cost, until_ms, complete, currency) = metered.map_or_else(
            || (0.0, 0, false, self.rates.currency.clone()),
            |reading| {
                (
                    reading.usage.cost(),
                    reading.usage.until_ms,
                    reading.usage.complete,
                    reading.currency.clone(),
                )
            },
        );
        let after: Vec<&CostSample> = samples.iter().filter(|s| s.at_ms >= until_ms).collect();
        let estimated = if complete {
            0.0
        } else {
            after
                .windows(2)
                .map(|pair| self.between(pair[0], pair[1]))
                .sum()
        };
        #[allow(clippy::cast_precision_loss)]
        let per_hour = match samples {
            [.., prev, last] if last.at_ms > prev.at_ms => {
                self.between(prev, last) * MS_PER_HOUR / (last.at_ms - prev.at_ms) as f64
            }
            _ => 0.0,
        };
        CostEstimate {
            metered: metered_cost,
            metered_until_ms: until_ms,
            complete,
            estimated,
            per_hour,
            currency,
        }
    }
}

/// The latest meter reading among ledger rows: the `usage` or
/// `destroyed` row with the latest `until_ms` (a complete one wins).
#[must_use]
pub fn latest_reading(rows: &[VmPayload]) -> Option<UsageReading> {
    rows.iter()
        .filter(|row| matches!(row.event, VmEvent::Usage | VmEvent::Destroyed))
        .filter_map(|row| row.usage.map(|usage| (usage, row.currency.clone())))
        .max_by_key(|(usage, _)| (usage.complete, usage.until_ms))
        .map(
            |(usage, currency): (VmUsage, Option<String>)| UsageReading {
                usage,
                currency: currency.unwrap_or_else(|| "eur".to_string()),
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3_600_000;

    fn sample(at_ms: u64, cpu: f64, mem_gib: u64, disk_gib: u64) -> CostSample {
        CostSample {
            at_ms,
            cpu_busy_secs: cpu,
            mem_used_bytes: mem_gib << 30,
            disk_used_bytes: disk_gib << 30,
        }
    }

    #[test]
    fn an_hour_at_known_use_costs_exactly_the_list_price() {
        let model = CostModel::new(PriceRates::boxd_list());
        // One full vCPU busy for an hour, 2 GiB in use, 20 GiB written.
        let cost = model.between(&sample(0, 0.0, 2, 20), &sample(HOUR, 3600.0, 2, 20));
        let expected = 0.039 + 2.0 * 0.012 + 20.0 * 0.000_08;
        assert!((cost - expected).abs() < 1e-12, "{cost} vs {expected}");
    }

    #[test]
    fn idle_reserved_cores_cost_nothing() {
        let model = CostModel::new(PriceRates::boxd_list());
        let cost = model.between(&sample(0, 100.0, 0, 0), &sample(HOUR, 100.0, 0, 0));
        assert!(cost.abs() < 1e-12);
    }

    #[test]
    fn the_estimate_starts_where_the_meter_ends() {
        let model = CostModel::new(PriceRates::boxd_list());
        let reading = UsageReading {
            usage: VmUsage {
                cost_micro: 410_000,
                until_ms: HOUR,
                ..VmUsage::default()
            },
            currency: "eur".into(),
        };
        let samples = [
            sample(0, 0.0, 2, 20),
            sample(HOUR, 3600.0, 2, 20),
            sample(2 * HOUR, 7200.0, 2, 20),
        ];
        let estimate = model.estimate(Some(&reading), &samples);
        let hour = 0.039 + 0.024 + 0.0016;
        assert!((estimate.metered - 0.41).abs() < 1e-12);
        assert!((estimate.estimated - hour).abs() < 1e-12, "{estimate:?}");
        assert!((estimate.per_hour - hour).abs() < 1e-12);
        assert!((estimate.total() - (0.41 + hour)).abs() < 1e-12);
        // Replaying the same inputs gives the same figure.
        assert_eq!(model.estimate(Some(&reading), &samples), estimate);
    }

    #[test]
    fn a_complete_reading_is_final() {
        let model = CostModel::new(PriceRates::boxd_list());
        let reading = UsageReading {
            usage: VmUsage {
                cost_micro: 1_000_000,
                complete: true,
                until_ms: 0,
                ..VmUsage::default()
            },
            currency: "usd".into(),
        };
        let estimate = model.estimate(
            Some(&reading),
            &[sample(0, 0.0, 1, 1), sample(HOUR, 3600.0, 1, 1)],
        );
        assert!(estimate.estimated.abs() < 1e-12);
        assert_eq!(estimate.currency, "usd");
    }

    #[test]
    fn overrides_apply() {
        let config = CloudConfig {
            price_vcpu_hour: Some(1.0),
            ..CloudConfig::defaults()
        };
        let model = CostModel::from_config(&config);
        assert!((model.rates().vcpu_hour - 1.0).abs() < f64::EPSILON);
        assert!((model.rates().mem_gib_hour - 0.012).abs() < f64::EPSILON);
    }

    #[test]
    fn the_latest_complete_reading_wins() {
        let row = |event, until_ms, cost_micro, complete| VmPayload {
            usage: Some(VmUsage {
                cost_micro,
                until_ms,
                complete,
                ..VmUsage::default()
            }),
            currency: Some("eur".into()),
            ..VmPayload::new(event, "id", "m")
        };
        let rows = vec![
            row(VmEvent::Usage, 10, 100, false),
            row(VmEvent::Destroyed, 20, 300, true),
            row(VmEvent::Usage, 30, 200, false),
            VmPayload::new(VmEvent::Hibernated, "id", "m"),
        ];
        assert_eq!(latest_reading(&rows).map(|r| r.usage.cost_micro), Some(300));
        assert_eq!(latest_reading(&[]), None);
    }
}
