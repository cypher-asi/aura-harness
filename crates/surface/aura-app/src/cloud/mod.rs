//! Cloud agents on boxd machines, from this app's side.
//!
//! The hub lives in [`aura_engine_cloud::hub`] and is re-exported here so
//! the app's paths stay put:
//!
//! - [`hub`]: the [`hub::CloudHub`] that moves agents to and from their
//!   machines, mirrors their records, drives their turns and routes
//!   their mail.
//! - [`store`]: the machine ledger writer and the record mirror's sink
//!   over the main store.
//! - [`plans`]: users' plan credentials from the vault, for their
//!   machines' boxd plan secrets.
//! - [`node_client`]: HTTP calls to one cloud node.
//! - [`setup`]: `aura cloud setup`, which puts a boxd API key in
//!   `AURA_HOME/.env`.
//!
//! [`host_info`] is the app's own: what the HOST tab shows, local or
//! cloud.

pub mod host_info;

pub use aura_engine_cloud::hub;
pub use aura_engine_cloud::hub::{node_client, plans, setup, store};

pub use hub::{CloudHub, CommitsOutcome, PatchOutcome, PlaceRequest, PullOutcome, RemoteEvent};
