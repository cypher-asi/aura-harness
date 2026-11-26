//! What benchmarks cost, how costs compare, and calls with no known price.

/// What follows a cost that leaves out calls with no known price.
pub const UNPRICED_MARK: &str = "+?";

/// A list-price cost and how many of the calls in it had no known price,
/// so that it is only what the others cost.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BenchCost {
    /// What the priced calls cost
    pub usd: f64,
    /// Calls with no known price, which `usd` leaves out
    pub unpriced_calls: u32,
}

impl BenchCost {
    /// `usd`, leaving out `unpriced_calls` calls.
    #[must_use]
    pub const fn new(usd: f64, unpriced_calls: u32) -> Self {
        Self {
            usd,
            unpriced_calls,
        }
    }

    /// Whether every call in it had a known price.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        self.unpriced_calls == 0
    }

    /// What it cost when every call had a price; `None` when some had
    /// none, so it cannot be set against another cost.
    #[must_use]
    pub const fn known(self) -> Option<f64> {
        if self.is_complete() {
            Some(self.usd)
        } else {
            None
        }
    }

    /// `money(usd)` when every call had a price, `$0.11+?` when some
    /// had none, and `unknown` when the priced ones cost nothing (or
    /// there were none).
    #[must_use]
    pub fn read(self, money: fn(f64) -> String) -> String {
        if self.is_complete() {
            money(self.usd)
        } else if self.usd <= 0.0 {
            "unknown".to_string()
        } else {
            money(self.usd) + UNPRICED_MARK
        }
    }
}

/// `+? some calls have no known price: glm-9, mystery` when `models`
/// made calls with no known price; `None` when there are none.
#[must_use]
pub fn unpriced_note<'a>(models: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut named: Vec<&str> = Vec::new();
    for model in models {
        if !named.contains(&model) {
            named.push(model);
        }
    }
    (!named.is_empty()).then(|| {
        format!(
            "{UNPRICED_MARK} some calls have no known price: {}",
            named.join(", ")
        )
    })
}

/// How two costs compare, for after each: ` · 100%` for the larger and
/// ` · 33% less` for the other, both ` · 100%` when they are about the
/// same. Empty when either is unknown or the larger is nothing.
#[must_use]
pub fn cost_shares(a: Option<f64>, b: Option<f64>) -> [String; 2] {
    let (Some(a), Some(b)) = (a, b) else {
        return [String::new(), String::new()];
    };
    let larger = a.max(b);
    if larger <= 0.0 {
        return [String::new(), String::new()];
    }
    let share = |cost: f64| {
        let less = ((1.0 - cost / larger) * 100.0).round();
        if less < 1.0 {
            " · 100%".to_string()
        } else {
            format!(" · {less:.0}% less")
        }
    };
    [share(a), share(b)]
}

/// What each verified fix cost: everything the arm spent, failed runs
/// included, over the runs whose check passed. `None` before a check
/// passes.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn cost_per_fix(total: BenchCost, fixes: usize) -> Option<BenchCost> {
    (fixes > 0).then(|| BenchCost::new(total.usd / fixes as f64, total.unpriced_calls))
}

/// The "cost per fix" cells of two arms: what each verified fix cost, with
/// how the two compare when both are complete, or `no fix` for an arm
/// none of whose checks passed.
#[must_use]
pub fn per_fix_cells(
    a: Option<BenchCost>,
    b: Option<BenchCost>,
    money: fn(f64) -> String,
) -> [String; 2] {
    let known = |cost: Option<BenchCost>| cost.and_then(BenchCost::known);
    let [a_share, b_share] = cost_shares(known(a), known(b));
    let cell = |cost: Option<BenchCost>, share: String| {
        cost.map_or_else(|| "no fix".to_string(), |cost| cost.read(money) + &share)
    };
    [cell(a, a_share), cell(b, b_share)]
}

/// What a replayed session cost when it ran.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchBaseline {
    pub calls: u32,
    pub cost_usd: f64,
    /// Whether any of its calls had a price
    pub priced: bool,
    /// Calls with no known price, which `cost_usd` leaves out
    pub unpriced_calls: u32,
    pub models: Vec<String>,
}

impl BenchBaseline {
    /// What the session cost as far as its calls had a price; `None`
    /// when none had one.
    #[must_use]
    pub fn cost(&self) -> Option<BenchCost> {
        self.priced
            .then(|| BenchCost::new(self.cost_usd, self.unpriced_calls))
    }
}
