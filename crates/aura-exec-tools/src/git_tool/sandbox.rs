//! Sandbox / `ToolContext` bridging helpers.
//!
//! The kernel guarantees `ToolContext::sandbox.root()` is the only path
//! the tool is allowed to mutate; the git tools enforce that by passing
//! `sandbox.root()` straight to the impl functions and never honoring a
//! caller-supplied `cwd`/`workspace`. The `tool_rejects_workspace_escape_via_config`
//! test in [`super::tests`] pins this behavior.
//!
//! These helpers turn the bag of `serde_json::Value` arguments + the
//! `ToolContext` into the typed values the impl functions actually
//! want — keeping that adapter layer out of the `Tool` impls themselves.

use std::time::Duration;

use super::{GitToolError, PushPolicy};
use crate::tool::ToolContext;

/// Per-operation timeout for non-push git operations (`add`, `commit`,
/// `diff --cached`, `rev-parse`).
///
/// Comfortably above the kernel's default `command_timeout` (10s) since
/// `git` shells out to a subprocess and can take longer on cold caches.
/// Tools still respect the hard upper bound via
/// `ToolConfig::max_async_timeout_ms`.
pub(super) fn workspace_timeout(ctx: &ToolContext) -> Duration {
    Duration::from_millis(ctx.config.max_async_timeout_ms.min(120_000))
}

/// Push-specific policy derived from the tool context.
///
/// Separate from [`workspace_timeout`] so slow `git push` calls can
/// use their own (longer, configurable) per-attempt budget and a
/// bounded retry loop without dragging the rest of the git tools up
/// with them. See [`PushPolicy::from_config`] for the knob.
pub(super) fn push_policy_for(ctx: &ToolContext) -> PushPolicy {
    PushPolicy::from_config(&ctx.config)
}

/// Extract a non-empty string argument from a `serde_json::Value` bag,
/// rejecting missing / non-string / empty values with
/// [`GitToolError::MissingArg`].
pub(super) fn str_arg<'a>(
    args: &'a serde_json::Value,
    name: &'static str,
) -> Result<&'a str, GitToolError> {
    args.get(name)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or(GitToolError::MissingArg(name))
}

/// Extract an optional boolean argument (`force` etc.). Defaults to
/// `false` when missing or wrongly typed.
pub(super) fn opt_bool(args: &serde_json::Value, name: &str) -> bool {
    args.get(name)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}
