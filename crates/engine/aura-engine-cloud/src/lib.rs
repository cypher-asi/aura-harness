//! # aura-engine-cloud
//!
//! Layer: engine
//!
//! Cloud agents on boxd machines (<https://boxd.sh>): each cloud agent's
//! kernel runs in an aura node on a boxd microVM of its own, and the
//! local app is the hub that creates, sleeps, wakes and destroys those
//! machines, mirrors their records and routes their mail.
//!
//! ## Surface
//!
//! - [`token`]: `BOXD_API_KEY` to a one-hour JWT, shared app-wide.
//! - [`provider`]: the [`VmProvider`] seam and its domain types;
//!   [`boxd`] implements it over boxd's gRPC API, [`fake`] in memory.
//! - [`fleet`]: the machine of every cloud agent and its lifecycle
//!   (create, claim, idle hibernation, wake, destroy, orphan sweep),
//!   recorded on ledger agents through a [`fleet::VmLedger`].
//! - [`snapshots`]: base and per-project snapshots holding a finished
//!   build and a running node, keyed by a fingerprint.
//! - [`glibc`]: the glibc a Linux aura binary needs versus the one the
//!   machines have, checked before a base snapshot is built.
//! - [`delta`]: the project files a machine gets on top of its snapshot,
//!   never including `.env` files.
//! - [`host`]: samples a host's name, addresses, CPU, memory and disk.
//! - [`machine_env`]: the allowlisted environment a machine gets.
//! - [`cost`]: metered usage plus a live estimate since the last reading.
//! - [`replica`]: checks and mirrors a node's record pages into the
//!   local store.
//! - [`mesh`]: the directory of which node holds which agent, mail ids
//!   and dedup.
//! - [`plans`]: each user's plan tokens as boxd host-bound secrets.
//! - [`github`]: the GitHub App client that mints one-repository
//!   installation tokens; [`repos`] keeps them in boxd host-bound
//!   secrets, one per repository.
//! - [`workspace`]: the shell that keeps a machine's workspace a real
//!   clone on its agent's branch, and the base it diffs against.
//! - [`names`]: machine and ledger names.
//! - [`boot`]: what a machine coming up shows while it happens, for the
//!   chat's boot console.
//!
//! ## Invariants
//!
//! - The boxd key and tokens never leave this process: machines get an
//!   explicit env allowlist, and node tokens travel only over boxd's
//!   authenticated `Exec`.
//! - The GitHub App key, App JWTs and repository tokens never leave this
//!   process either: machines hold only `bxds_` placeholders.
//! - Every external call has a timeout (rules.md §6.2).

pub mod boot;
pub mod boxd;
pub mod cost;
pub mod delta;
pub mod error;
pub mod fake;
pub mod fleet;
pub mod github;
pub mod glibc;
pub mod host;
pub mod machine_env;
pub mod mesh;
pub mod names;
pub mod plans;
pub mod provider;
pub mod replica;
pub mod repos;
pub mod snapshots;
pub mod token;
pub mod workspace;

pub mod proto {
    //! Code generated from boxd's vendored `api.proto` (`boxd.api.v1`).
    #![allow(
        clippy::all,
        clippy::pedantic,
        clippy::nursery,
        missing_docs,
        rustdoc::all,
        unreachable_pub
    )]
    tonic::include_proto!("boxd.api.v1");
}

pub use boot::{BootEvent, BootReporter, BootStage, StageState};
pub use boxd::BoxdProvider;
pub use error::{CloudError, CloudResult};
pub use fake::FakeProvider;
pub use provider::{
    Balance, ExecOutput, HostSecret, MachineInfo, MachineLimit, MachineOrigin, MachineStatus,
    NewMachine, OrgUsage, SnapshotInfo, UsageReading, VmProvider,
};
pub use token::TokenSource;
