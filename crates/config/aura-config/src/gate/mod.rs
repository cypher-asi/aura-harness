//! How Gate decides a model has proven itself, how often it tries a
//! cheaper one, whether the lead agent must delegate its work, whether a
//! judge model reviews routed turns that failed, and what Gate does
//! about a routed turn that ran no check before its judge gives the
//! verdict.
//!
//! ## Owned env vars
//!
//! | Var | Type | Default | Field |
//! | --- | --- | --- | --- |
//! | `AURA_GATE_TARGET` | f64 (0..=1) | `0.85` | [`GateConfig::target`] |
//! | `AURA_GATE_MIN_TRIES` | u32 (>= 1) | `10` | [`GateConfig::min_tries`] |
//! | `AURA_GATE_EXPLORE` | f64 (0..=1) | `0.05` | [`GateConfig::explore_rate`] |
//! | `AURA_GATE_DELEGATE` | `on` / `off` | `on` | [`GateConfig::delegate`] |
//! | `AURA_GATE_JUDGE` | `on` / `off` | `on` | [`GateConfig::judge`] |
//! | `AURA_GATE_NUDGE` | `on` / `off` | `on` | [`GateConfig::nudge`] |
//! | `AURA_GATE_CHECK` | `on` / `off` | `on` | [`GateConfig::harness_check`] |
//! | `AURA_GATE_REVIEW` | `on` / `log` / `off` | `log` | [`GateConfig::review`] |
//! | `AURA_GATE_CRITERIA` | `on` / `log` / `off` | `log` | [`GateConfig::criteria`] |
//! | `AURA_GATE_REVIEW_SAMPLE` | f64 (0..=1) | `0.25` | [`GateConfig::review_sample`] |
//!
//! A value outside what a variable accepts keeps that setting's default
//! and logs a warning naming the variable and its accepted values; the
//! other settings, Gate's and the rest of `aura-config`'s, still apply.

mod env;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};

const DEFAULT_TARGET: f64 = 0.85;
const DEFAULT_MIN_TRIES: u32 = 10;
const DEFAULT_EXPLORE_RATE: f64 = 0.05;
const DEFAULT_REVIEW_SAMPLE: f64 = 0.25;

/// What Gate's judge does about work a routed turn is about to let
/// stand.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMode {
    /// It is not asked.
    Off,
    /// It reviews the work and its verdict is logged, but the turn goes
    /// on as if it had not been asked.
    #[default]
    Log,
    /// It reviews the work, and work it finds unfinished goes back to
    /// the model once, then on to a stronger one.
    On,
}

impl Display for ReviewMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Off => "off",
            Self::Log => "log",
            Self::On => "on",
        })
    }
}

/// Gate's routing settings.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "snake_case")]
pub struct GateConfig {
    /// Success rate a model needs on a category to become its pick.
    ///
    /// env: `AURA_GATE_TARGET` (default: `0.85`)
    pub target: f64,
    /// Counted outcomes a model needs on a category before its success
    /// rate is trusted.
    ///
    /// env: `AURA_GATE_MIN_TRIES` (default: `10`)
    pub min_tries: u32,
    /// Share of routed work sent to the next cheaper model instead of
    /// the pick, to learn how it does.
    ///
    /// env: `AURA_GATE_EXPLORE` (default: `0.05`)
    pub explore_rate: f64,
    /// Whether the lead agent under Gate must delegate: no file edits and
    /// only check and inspection commands of its own. When off it keeps
    /// every tool and is only told to delegate.
    ///
    /// env: `AURA_GATE_DELEGATE` (default: `on`)
    pub delegate: bool,
    /// Whether a judge model reviews each turn Gate routed that failed
    /// (an error, or its last test or build failing) and says whether the
    /// model was at fault, which decides whether the failure counts
    /// against it and the turn moves to a stronger model. When off, every
    /// such failure counts and moves the turn up. Work that ran no check
    /// is judged either way: the judge's verdict is its outcome.
    ///
    /// env: `AURA_GATE_JUDGE` (default: `on`)
    pub judge: bool,
    /// Whether a routed turn below the strongest tier that changed files
    /// but ran no test or build is asked once, on the same model, to run
    /// the tests that cover its change.
    ///
    /// env: `AURA_GATE_NUDGE` (default: `on`)
    pub nudge: bool,
    /// Whether the harness runs the project's tests (`AURA_DOD_TEST_COMMAND`,
    /// else the command its manifest implies) on a routed turn below the
    /// strongest tier that changed files and still ran none. A failure
    /// moves the turn up as a failed check of its own does.
    ///
    /// env: `AURA_GATE_CHECK` (default: `on`)
    pub harness_check: bool,
    /// Whether the judge reviews work a routed turn below the strongest
    /// tier is about to let stand, when the work was classified hard or
    /// shows a sign of failure the attempt log backs: it sees the request,
    /// everything the turn changed, its checks and its reply. With `on`,
    /// work it finds unfinished goes back to the same model once with
    /// what it found, then on to a stronger model. With `log` its verdict
    /// is only written to the attempt log. Needs [`Self::judge`] on as
    /// well.
    ///
    /// env: `AURA_GATE_REVIEW` (default: `log`)
    pub review: ReviewMode,
    /// Whether a routed code turn's acceptance criteria are written down,
    /// by a model other than the one doing the work, before the work
    /// starts, and the work is reviewed against them before it stands.
    /// With `on` and `log` alike the worker is shown them after its
    /// prompt, and criteria that ask for output to match exactly make the
    /// work hard. With `on`, work the review finds short of them goes
    /// back to the model once, then on to a stronger one. With `log` the
    /// review's verdict only goes to the attempt log. Needs
    /// [`Self::judge`] on.
    ///
    /// env: `AURA_GATE_CRITERIA` (default: `log`)
    pub criteria: ReviewMode,
    /// Share of routed turns whose work gets a review that is only
    /// logged, with [`Self::review`] or [`Self::criteria`] at `log`; the
    /// others skip it. A review Gate acts on is never skipped. A logged
    /// review changes nothing about the turn it costs a call on, and a
    /// sample says as much about how often the reviewer is right.
    ///
    /// env: `AURA_GATE_REVIEW_SAMPLE` (default: `0.25`)
    pub review_sample: f64,
}

impl GateConfig {
    /// Compile-time defaults. No env access.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            target: DEFAULT_TARGET,
            min_tries: DEFAULT_MIN_TRIES,
            explore_rate: DEFAULT_EXPLORE_RATE,
            delegate: true,
            judge: true,
            nudge: true,
            harness_check: true,
            review: ReviewMode::Log,
            criteria: ReviewMode::Log,
            review_sample: DEFAULT_REVIEW_SAMPLE,
        }
    }

    /// These settings, never exploring: work goes to the pick every
    /// time, as where a choice must not change from one run to the next.
    #[must_use]
    pub const fn without_exploring(self) -> Self {
        Self {
            explore_rate: 0.0,
            ..self
        }
    }

    /// Apply env overrides. A variable set to a value it does not accept
    /// keeps its default, with a warning naming it and what it accepts.
    #[must_use]
    pub fn from_env() -> Self {
        let (cfg, rejected) = Self::read_env();
        for err in &rejected {
            tracing::warn!(error = %err, "aura-config: ignoring an invalid Gate setting");
        }
        cfg
    }
}

impl Default for GateConfig {
    fn default() -> Self {
        Self::defaults()
    }
}
