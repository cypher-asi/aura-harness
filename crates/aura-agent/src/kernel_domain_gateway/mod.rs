//! Kernel-mediated gateway for [`DomainApi`].
//!
//! [`KernelDomainGateway`] wraps an `Arc<dyn DomainApi>` and implements
//! the same trait. Read-only methods (`list_*`, `get_*`) are passed
//! through directly — they are declared exceptions per
//! `docs/invariants.md` §1. Every mutating method records a pre-call
//! "request snapshot" and a post-call "response" `RecordEntry` by
//! routing a [`Transaction`](aura_core::Transaction) with
//! [`TransactionType::System`](aura_core::TransactionType::System) and
//! [`SystemKind::DomainMutation`](aura_core::SystemKind::DomainMutation)
//! through [`Kernel::process_direct`](aura_kernel::Kernel::process_direct).
//!
//! The gateway satisfies Invariant §2 ("every state change is a
//! transaction") and §8 ("gateway transparency") for the domain API
//! surface consumed by automatons.
//!
//! # Module layout
//!
//! - [`handle`] — the [`KernelDomainGateway`] struct, constructor, and the
//!   private `record_request` / `record_response` helpers that emit the
//!   pre/post `RecordEntry` pair around every mutating call.
//! - [`wire`] — the [`KernelDomainGatewayError`] enum and the
//!   `with_recording!` / `with_recording_unit!` macros that bracket inner
//!   calls with the recording pair so each `DomainApi` method stays a
//!   one-liner.
//! - [`routes`] — the `DomainApi` trait `impl` itself: every method
//!   classifies itself as read-only (passthrough) or mutating (recorded)
//!   and forwards to the inner provider.
//! - [`tests`] — moved-out unit tests covering both the read-only
//!   passthrough and the request/response recording invariants.
//!
//! # Error handling
//!
//! - The HTTP timeout enforced by the concrete `DomainApi`
//!   implementation (`HttpDomainApi`) still applies because we call
//!   through the inner `Arc<dyn DomainApi>` directly.
//! - Failures from the kernel's `process_direct` call (e.g. store
//!   corruption) are logged at `error!` level but do not mask the
//!   underlying domain error on the mutation's result value.

mod handle;
mod routes;
mod wire;

#[cfg(test)]
mod tests;

pub use handle::KernelDomainGateway;
pub use wire::KernelDomainGatewayError;
