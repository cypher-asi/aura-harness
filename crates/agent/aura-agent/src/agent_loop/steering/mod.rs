//! Steering shim — re-exports the agent-steering crate.
//!
//! Phase 6a moved every stateful evaluator (`RepeatedReadTracker`,
//! `ImplementNowSteering`, `EarlyTestOracle`) plus the
//! `TurnSteering` trait and `SteeringRegistry` into the dedicated
//! `aura-agent-steering` crate so the steering layer sits strictly
//! below the agent loop in the layer order.
//!
//! This module keeps the `crate::agent_loop::steering::*` paths the
//! agent loop uses. New code should import from `aura_agent_steering`
//! directly.

pub use aura_agent_steering::{inject, SteeringRegistry};
