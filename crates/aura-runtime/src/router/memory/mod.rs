//! Memory CRUD API endpoints for facts, events, procedures, and aggregates.
//!
//! Split into:
//! - [`wire`] — request body structs (`CreateFactBody`, …) the
//!   handlers deserialize from JSON.
//! - [`handlers`] — the `pub(super) async fn` HTTP handlers + their
//!   shared `ApiResult` / id-parsing helpers.
//!
//! `mod.rs` re-exports the handler set with `pub(super) use` so the
//! parent router (`router::build::create_router`) can keep mounting
//! every endpoint via `memory::list_facts`, `memory::create_event`,
//! etc. — exactly the path it used before this split.

mod handlers;
mod wire;

pub(super) use handlers::{
    bulk_delete_events, consolidate, create_event, create_fact, create_procedure, delete_event,
    delete_fact, delete_procedure, get_fact, get_fact_by_key, get_procedure, list_events,
    list_facts, list_procedures, snapshot, stats, update_fact, update_procedure, wipe,
};
