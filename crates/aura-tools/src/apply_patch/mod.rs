//! `apply_patch`: codex-envelope multi-file patch tool.
//!
//! The harness-v2.2 dev-loop exposes a single write primitive,
//! `apply_patch`, which takes a multi-file diff in the format codex-rs
//! popularised:
//!
//! ```text
//! *** Begin Patch
//! *** Add File: path/new.rs
//! +file content
//! *** Update File: path/existing.rs
//! @@ optional context @@
//!  unchanged context
//! -removed line
//! +added line
//! *** Delete File: path/old.rs
//! *** End Patch
//! ```
//!
//! A single call may carry any combination of Add/Update/Delete
//! directives across multiple files. Semantics are atomic-ish: every
//! directive is validated against the current filesystem state (target
//! existence + hunk context matches) before *any* file is mutated. If
//! validation fails, none of the changes are applied and the caller
//! gets a structured error pointing at the offending file/hunk.
//!
//! ## Layering
//!
//! - [`parser`]: pure string -> [`Patch`] / [`PatchError`]. Knows
//!   nothing about filesystems.
//! - [`executor`]: validate-all-then-apply-all over a workspace root.
//!   Returns [`AppliedChange`] records the caller can translate into
//!   the agent layer's `FileChange` / `FileOp` types.
//!
//! Higher layers (`aura-agent::task_executor`) wire the actual tool
//! handler that parses the model's `patch` argument, drives this
//! executor against the sandbox root, and folds the resulting
//! [`AppliedChange`]s back into the agent's tracked-file-ops pipeline
//! so Phase B's `had_any_file_write` flag fires naturally.

mod executor;
mod parser;

pub use executor::{
    execute_apply_patch, AppliedChange, AppliedChangeKind, ApplyPatchError, ApplyPatchResult,
};
pub use parser::{parse_patch, FileChange, Hunk, HunkLine, Patch, PatchError};
