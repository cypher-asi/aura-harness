//! Gate's settings read from the environment.

use super::{GateConfig, ReviewMode};
use crate::env::{
    lookup_string, AURA_GATE_CHECK, AURA_GATE_CRITERIA, AURA_GATE_DELEGATE, AURA_GATE_EXPLORE,
    AURA_GATE_JUDGE, AURA_GATE_MIN_TRIES, AURA_GATE_NUDGE, AURA_GATE_REVIEW,
    AURA_GATE_REVIEW_SAMPLE, AURA_GATE_TARGET, FALSY_LITERALS, TRUTHY_LITERALS,
};
use crate::ConfigError;
use std::fmt::Display;

pub(super) const RATE_ACCEPTS: &str = "a number from 0 to 1";
pub(super) const MIN_TRIES_ACCEPTS: &str = "a whole number of at least 1";
pub(super) const SWITCH_ACCEPTS: &str = "on or off (also 1/0, true/false, yes/no)";
pub(super) const REVIEW_ACCEPTS: &str = "on, log or off (also 1/0, true/false, yes/no)";

impl GateConfig {
    /// The settings from the environment, and the variables whose value
    /// was not accepted, in the order they are read.
    pub(super) fn read_env() -> (Self, Vec<ConfigError>) {
        let mut cfg = Self::defaults();
        let mut env = Reader::default();
        env.rate(AURA_GATE_TARGET, &mut cfg.target);
        env.min_tries(AURA_GATE_MIN_TRIES, &mut cfg.min_tries);
        env.rate(AURA_GATE_EXPLORE, &mut cfg.explore_rate);
        env.switch(AURA_GATE_DELEGATE, &mut cfg.delegate);
        env.switch(AURA_GATE_JUDGE, &mut cfg.judge);
        env.switch(AURA_GATE_NUDGE, &mut cfg.nudge);
        env.switch(AURA_GATE_CHECK, &mut cfg.harness_check);
        env.review(AURA_GATE_REVIEW, &mut cfg.review);
        env.review(AURA_GATE_CRITERIA, &mut cfg.criteria);
        env.rate(AURA_GATE_REVIEW_SAMPLE, &mut cfg.review_sample);
        (cfg, env.rejected)
    }
}

/// Reads variables into the settings they set, noting each value it does
/// not accept.
#[derive(Default)]
struct Reader {
    rejected: Vec<ConfigError>,
}

impl Reader {
    fn rate(&mut self, name: &'static str, setting: &mut f64) {
        let default = *setting;
        self.read(name, RATE_ACCEPTS, &default, parse_rate, setting);
    }

    fn min_tries(&mut self, name: &'static str, setting: &mut u32) {
        let default = *setting;
        self.read(name, MIN_TRIES_ACCEPTS, &default, parse_min_tries, setting);
    }

    fn switch(&mut self, name: &'static str, setting: &mut bool) {
        let default = if *setting { "on" } else { "off" };
        self.read(name, SWITCH_ACCEPTS, &default, parse_switch, setting);
    }

    fn review(&mut self, name: &'static str, setting: &mut ReviewMode) {
        let default = *setting;
        self.read(name, REVIEW_ACCEPTS, &default, parse_review, setting);
    }

    /// Set `setting` from `name` when it is set and `parse` accepts its
    /// value. A value it rejects is noted, saying what `name` accepts and
    /// that `default` stays.
    fn read<T>(
        &mut self,
        name: &'static str,
        accepts: &str,
        default: &dyn Display,
        parse: fn(&str) -> Option<T>,
        setting: &mut T,
    ) {
        let Some(raw) = lookup_string(name) else {
            return;
        };
        match parse(&raw) {
            Some(value) => *setting = value,
            None => self.rejected.push(ConfigError::InvalidValue {
                name,
                raw,
                message: format!("expected {accepts}; using the default {default}"),
            }),
        }
    }
}

fn parse_rate(raw: &str) -> Option<f64> {
    raw.parse::<f64>().ok().filter(|v| (0.0..=1.0).contains(v))
}

fn parse_min_tries(raw: &str) -> Option<u32> {
    raw.parse::<u32>().ok().filter(|v| *v >= 1)
}

fn parse_review(raw: &str) -> Option<ReviewMode> {
    if raw.eq_ignore_ascii_case("log") {
        return Some(ReviewMode::Log);
    }
    parse_switch(raw).map(|on| if on { ReviewMode::On } else { ReviewMode::Off })
}

fn parse_switch(raw: &str) -> Option<bool> {
    let matches = |literals: &[&str]| literals.iter().any(|l| l.eq_ignore_ascii_case(raw));
    if matches(TRUTHY_LITERALS) {
        Some(true)
    } else if matches(FALSY_LITERALS) {
        Some(false)
    } else {
        None
    }
}
