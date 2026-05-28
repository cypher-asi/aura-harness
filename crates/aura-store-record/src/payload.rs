//! `RecordPayload` — placeholder for the audited-payload representation.
//!
//! Phase 6a populates the [`RecordPayload::Summary`] variant for
//! `KernelMode::AuditedLite` (head/tail bytes plus a full-payload
//! content hash). Phase 2 ships the [`RecordPayload::Inline`]-only
//! stub so the wire shape is reserved without forcing every caller
//! to handle a richer enum yet.
//!
//! ## Wire format
//!
//! Externally tagged via `#[serde(rename_all = "snake_case")]` —
//! Phase 2's only variant therefore serialises as
//! `{"inline": [..bytes..]}`. Internal tagging (`#[serde(tag =
//! "kind")]`) is incompatible with serde's tuple variants, so the
//! payload uses external tagging while [`crate::RecordKind`] (whose
//! variants are all unit) keeps the more compact internal-tag form.
//! Both shapes coexist on the wire because the kernel writes them
//! into separate JSON fields of [`crate::RecordEntry`].
//!
//! ## Invariants (per `.cursor/rules.md` §13)
//!
//! - [`RecordPayload::Inline`] always carries the entire payload. It
//!   is the unconditional shape for `KernelMode::Audited` and for
//!   any record below the per-mode size threshold.
//!
//! ## Failure modes
//!
//! Phase 2 has no failure modes — the type is purely structural. The
//! `Summary` variant arrives in Phase 6a and may surface
//! `SnapshotMissing` errors at replay time when paired with
//! `aura-store-snapshot`'s real backend.

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Audited-payload representation. Phase 2 ships only the `Inline`
/// variant; Phase 6a adds `Summary` for `KernelMode::AuditedLite`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordPayload {
    /// Full inline payload bytes. Default and only Phase 2 shape.
    Inline(#[serde(with = "bytes_serde")] Bytes),
    // Phase 6a: Summary { head, tail, full_hash, full_len } gates
    // large payloads to avoid blowing audit cost in
    // `KernelMode::AuditedLite`.
}

impl RecordPayload {
    /// Construct an [`RecordPayload::Inline`] from any byte source.
    #[must_use]
    pub fn inline(bytes: impl Into<Bytes>) -> Self {
        Self::Inline(bytes.into())
    }
}

/// Serde adapter for `bytes::Bytes` that round-trips through a
/// `Vec<u8>`. Avoids pulling in `serde_bytes` for a single use site.
mod bytes_serde {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S: Serializer>(value: &Bytes, ser: S) -> Result<S::Ok, S::Error> {
        value.as_ref().to_vec().serialize(ser)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Bytes, D::Error> {
        let raw: Vec<u8> = Vec::deserialize(de)?;
        Ok(Bytes::from(raw))
    }
}
