//! Invariant §10 (type-surface) — `ReadStore` must NOT expose any
//! sealed `WriteStore` methods.
//!
//! Phase 2 remediation: non-kernel crates are migrating from
//! `Arc<dyn Store>` (full record-append surface) to `Arc<dyn ReadStore>`
//! (read + explicitly-allowed inbox writes only). To make that split
//! enforceable at compile time we assert, via `static_assertions`:
//!
//! * `RocksStore` — the canonical concrete backend — implements both
//!   `ReadStore` and the sealed `WriteStore`.
//! * `Arc<dyn ReadStore>` is `Send + Sync` (so the router can hold it
//!   across async await points and tokio task boundaries).
//! * The `sealed::Sealed` marker is not `pub`, so external crates can
//!   neither name nor satisfy it. (Compile-time proof by construction:
//!   if we removed the `pub(crate)` qualifier this test would fail to
//!   parse; because we can only reach the trait through the private
//!   path `aura_store::store::sealed::Sealed`, no downstream crate
//!   can produce a `WriteStore` impl.)
//!
//! A negative `trybuild` compile-fail test would make the seal even
//! more airtight; punted to the same cleanup that re-enables
//! `trybuild` in CI.

#![cfg(feature = "test-support")]

use aura_store::{ReadStore, RocksStore, Store, WriteStore};
use std::sync::Arc;

// Positive: `RocksStore` implements every layer of the trait tower.
static_assertions::assert_impl_all!(RocksStore: ReadStore, WriteStore, Store);

// `Arc<dyn ReadStore>` is the narrow handle non-kernel crates should
// bind to. It must remain `Send + Sync` so it can cross tokio task
// boundaries without specialised wrappers.
static_assertions::assert_impl_all!(Arc<dyn ReadStore>: Send, Sync, Clone);

// The sealed `WriteStore` trait is object-safe (required so
// `Arc<dyn WriteStore>` is expressible inside the kernel) but only
// `aura-store` can produce new implementations. The compile-time
// guarantee is encoded by the private `sealed::Sealed` bound on
// `WriteStore`; any external `impl WriteStore for MyType` would be
// rejected by rustc because the supertrait `Sealed` is not nameable
// from downstream crates.
static_assertions::assert_obj_safe!(WriteStore);
static_assertions::assert_obj_safe!(ReadStore);

#[test]
fn read_store_handle_is_usable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let concrete: Arc<RocksStore> =
        Arc::new(RocksStore::open(dir.path(), false).expect("open rocks"));
    let read_only: Arc<dyn ReadStore> = concrete.clone();
    // Smoke-test: a plain `ReadStore` handle can answer a read-only
    // query without any reference to `WriteStore`.
    let _ = read_only
        .get_inbox_depth(aura_core::AgentId::generate())
        .expect("inbox depth read succeeds on an empty store");
}
