//! Request-body wire shapes consumed by the memory CRUD handlers.
//!
//! Kept separate from `handlers.rs` so a future schema change (e.g.
//! versioned bodies, new optional fields, alternate fact sources)
//! does not have to walk through several hundred lines of HTTP
//! plumbing to find the type definitions.

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Deserialize)]
pub(in crate::gateway) struct CreateFactBody {
    pub key: String,
    pub value: serde_json::Value,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default = "default_importance")]
    pub importance: f32,
}

pub(in crate::gateway) fn default_confidence() -> f32 {
    1.0
}

pub(in crate::gateway) fn default_importance() -> f32 {
    0.5
}

#[derive(Deserialize)]
pub(in crate::gateway) struct CreateEventBody {
    pub event_type: String,
    pub summary: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default = "default_importance")]
    pub importance: f32,
}

#[derive(Deserialize)]
pub(in crate::gateway) struct BulkDeleteEventsBody {
    pub before: DateTime<Utc>,
}

#[derive(Deserialize, Default)]
pub(in crate::gateway) struct ProcedureListParams {
    pub skill: Option<String>,
    pub min_relevance: Option<f32>,
}

#[derive(Deserialize)]
pub(in crate::gateway) struct CreateProcedureBody {
    pub name: String,
    pub trigger: String,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub context_constraints: serde_json::Value,
    #[serde(default)]
    pub skill_name: Option<String>,
    #[serde(default)]
    pub skill_relevance: Option<f32>,
}

#[derive(Deserialize)]
pub(in crate::gateway) struct UpdateProcedureBody {
    pub name: String,
    pub trigger: String,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub context_constraints: serde_json::Value,
    pub skill_name: Option<String>,
    pub skill_relevance: Option<f32>,
}
