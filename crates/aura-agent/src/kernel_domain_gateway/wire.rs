//! Recording-payload wire surface and the bracketing macros.
//!
//! The `KernelDomainGateway` records a `request` entry before each
//! mutating `DomainApi` call and a `response` entry after. Both entries
//! serialize through [`Kernel::process_direct`](aura_kernel::Kernel::process_direct)
//! as `System` / `DomainMutation` payloads. This module owns the JSON
//! shape of those payloads (via the `with_recording!` /
//! `with_recording_unit!` macros consumed by the trait `impl` in
//! [`super::routes`]) and the public [`KernelDomainGatewayError`] type.

/// Errors emitted by the gateway when it fails to construct or submit
/// the mandatory `System`/`DomainMutation` record entries. These are
/// logged but do not replace the original mutation result; they exist
/// so internal call sites (tests in particular) can distinguish a
/// store-layer failure from a transport failure.
#[derive(Debug, thiserror::Error)]
pub enum KernelDomainGatewayError {
    /// The kernel refused to append the recording transaction.
    #[error("kernel recording failed: {0}")]
    Kernel(String),
    /// Serialization of the recording payload failed.
    #[error("recording payload serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Helper macro: brackets an `inner` mutating call with pre/post
/// record entries. Returns the inner result verbatim.
macro_rules! with_recording {
    ($self:ident, $method:expr, $args:expr, $call:expr) => {{
        let method: &'static str = $method;
        $self.record_request(method, $args).await;
        let result = $call.await;
        let (ok, err_msg) = match &result {
            Ok(_) => (true, None),
            Err(e) => (false, Some(e.to_string())),
        };
        $self.record_response(method, ok, err_msg).await;
        result
    }};
}

/// Helper macro for mutating calls returning `anyhow::Result<()>`.
macro_rules! with_recording_unit {
    ($self:ident, $method:expr, $args:expr, $call:expr) => {{
        let method: &'static str = $method;
        $self.record_request(method, $args).await;
        let result = $call.await;
        let (ok, err_msg) = match &result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e.to_string())),
        };
        $self.record_response(method, ok, err_msg).await;
        result
    }};
}

pub(super) use with_recording;
pub(super) use with_recording_unit;
