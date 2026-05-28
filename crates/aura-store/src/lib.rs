//! # aura-store (compatibility shell)
//!
//! Layer: store (shell)
//!
//! Phase 2: this crate's implementation moved to `aura-store-db`.
//! Re-exported here so existing call sites keep compiling without
//! a flag-day rename. New code should depend on `aura-store-db`
//! directly.
//!
//! The shell preserves the legacy public surface verbatim:
//!
//! - `aura_store::RocksStore` → `aura_store_db::RocksStore`
//! - `aura_store::Store` / `ReadStore` / `WriteStore`
//! - `aura_store::StoreError`, key codecs, column-family constants.
//!
//! When `test-support` is enabled this re-export also surfaces the
//! `FaultAt` fault-injection helper used by the Wave 7 atomicity
//! suite.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

pub use aura_store_db::*;
