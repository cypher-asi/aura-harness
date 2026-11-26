//! Cloud agents on boxd machines, from this app's side.
//!
//! - [`hub`]: the [`hub::CloudHub`] that moves agents to and from their
//!   machines, mirrors their records, drives their turns and routes
//!   their mail.
//! - [`store`]: the machine ledger writer and the record mirror's sink
//!   over the main store.
//! - [`plans`]: users' plan credentials from the vault, for their
//!   machines' boxd plan secrets.
//! - [`node_client`]: HTTP calls to one cloud node.
//! - [`host_info`]: what the HOST tab shows, local or cloud.
//! - [`setup`]: `aura cloud setup`, which puts a boxd API key in
//!   `AURA_HOME/.env`.

pub mod host_info;
pub mod hub;
pub mod node_client;
pub mod plans;
pub mod setup;
pub mod store;
#[cfg(test)]
pub(crate) mod test_node;

pub use hub::{CloudHub, CommitsOutcome, PatchOutcome, PlaceRequest, PullOutcome, RemoteEvent};
