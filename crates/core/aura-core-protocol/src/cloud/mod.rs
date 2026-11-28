//! Wire types between the hub (the local app) and the aura nodes running
//! on cloud machines, and between those nodes.
//!
//! - [`mesh`]: frames on the long-lived `WS /v1/mesh` socket (mail,
//!   directory, record-commit hints, host stats).
//! - [`sync`]: bodies of the `/v1/sync/*`, `/v1/node/*` and
//!   `/v1/workspace/patch` calls the hub makes to move an agent's
//!   single writer and pull its work back.
//! - [`host`]: what `GET /v1/host` reports about the machine a node runs
//!   on, also used for this machine in the HOST tab.
//!
//! These are internal to the harness (hub and nodes run the same
//! binary), so they carry no TypeScript bindings. Every enum is tagged
//! with `type` so unknown frames from a newer node are skipped rather
//! than misread ([`mesh::MeshFrame::Unknown`]).

pub mod host;
pub mod mesh;
pub mod sync;

pub use host::{HostAddress, HostStats, SnapshotOrigin};
pub use mesh::{
    MeshAgent, MeshFrame, MeshMail, MeshMailKind, MeshPeer, MeshProcess, MESH_PROCESS_AGENT,
    MESH_PROTOCOL,
};
pub use sync::{
    GitClaim, NodeClaim, NodeClaimed, PlanTemplate, RecordPage, SealResponse, SessionSaved,
    SessionStarted, SyncImport, SyncImported, TracePage, WorkspacePatch, NODE_PROTOCOL,
    PLACEHOLDER_PREFIX, SHIP_TOOL,
};
