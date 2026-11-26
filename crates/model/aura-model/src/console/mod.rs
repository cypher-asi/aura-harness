//! Visual block helpers for Anthropic request / response logging.
//!
//! These mirror the structure in `aura-agent::console`: every block is
//! a self-contained multi-line string emitted through `tracing::info!`
//! with the dedicated `"aura::console"` target, so the custom event
//! formatter in `aura-node` can render it verbatim without level /
//! target / field noise.
//!
//! The forensic per-field `INFO` logs in
//! `crates/aura-reasoner/src/anthropic/provider.rs` were demoted to
//! `debug!` as part of the same change — operators get a clean
//! transcript by default and can opt back into the full field dump
//! with `RUST_LOG=aura_model=debug`.
//!
//! ## Layout & colors
//!
//! See the matching module doc on `aura-agent::console` for the
//! shared layout / color philosophy. The row / value helpers live in
//! [`layout`] and are shared with `aura-agent::console` and
//! `aura-node::inbound_console`.

pub mod layout;

use colored::Colorize;
use tracing::info;

use crate::types::ModelResponse;
use layout::{collapse_for_row, destination_tag, human_bytes, human_duration_ms, wrap_row};

/// Tracing target the runtime's custom formatter renders verbatim.
pub const CONSOLE_TARGET: &str = "aura::console";

/// View into the request data the multi-line block needs. Borrowing
/// avoids a string-clone storm at the call site (the values come
/// directly from the existing `RequestDiagnosticsSummary` plus a few
/// derived labels).
pub struct AnthropicRequestView<'a> {
    pub model: &'a str,
    pub kind: &'a str,
    pub body_bytes: usize,
    pub messages_count: usize,
    pub tools_count: usize,
    pub tool_choice: &'a str,
    /// Comma-joined list of tool names attached to the request, taken
    /// straight from the serialized body. Empty when the request
    /// shipped without any tools (e.g. `kind: Auxiliary` sub-prompts);
    /// the renderer skips the `tool_names` row in that case so the
    /// block stays tight.
    pub tool_names: &'a str,
    pub thinking_label: &'a str,
    pub system_bytes: usize,
    pub last_user_bytes: usize,
    pub last_user_hash: Option<&'a str>,
    pub headers_present: &'a str,
    pub request_hash: &'a str,
    /// Short semantic label for the network destination (e.g.
    /// `"aura-network"` for any call routed through the LLM proxy).
    pub destination: &'a str,
    /// Host portion of the target URL (e.g.
    /// `"aura-router.onrender.com"`).
    pub destination_host: &'a str,
}

/// Mirror view for the response side. Populated from the wire status,
/// `response.usage`, and the wall-clock elapsed time the provider
/// already measures.
pub struct AnthropicResponseView<'a> {
    pub status_code: u16,
    pub status_text: &'a str,
    pub stop_reason: &'a str,
    pub elapsed_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub request_id: Option<&'a str>,
    /// Short semantic label for the network destination (e.g.
    /// `"aura-network"`). Host is omitted on the response side to
    /// keep the header line scannable — the request header above
    /// already carries the full host.
    pub destination: &'a str,
}

/// Render the request half of an Anthropic call as a multi-line block.
pub fn anthropic_request_block(req: &AnthropicRequestView<'_>) {
    let mut out = String::new();
    let header = "→ POST /v1/messages".cyan().bold();
    let tag = destination_tag(req.destination, Some(req.destination_host));
    out.push_str(&format!("{} {header}  {tag}\n", "┌─".dimmed()));
    out.push_str(&wrap_row("model", req.model));
    out.push_str(&wrap_row("kind", req.kind));
    out.push_str(&wrap_row("body", &human_bytes(req.body_bytes)));
    out.push_str(&wrap_row("msgs", &req.messages_count.to_string()));
    out.push_str(&wrap_row(
        "tools",
        &format!("{} ({})", req.tools_count, req.tool_choice),
    ));
    if !req.tool_names.is_empty() {
        out.push_str(&wrap_row("tool_names", req.tool_names));
    }
    out.push_str(&wrap_row(
        "thinking",
        // Widen the inline padding to fit the richer label format
        // (e.g. `on(medium · enabled · b=4096)`); the legacy 14-col
        // pad was sized for `on` / `off` and would smash the system
        // / last_user tail against the label on every thinking turn.
        &format!(
            "{:<32} system {:<6} last_user {} / {}",
            req.thinking_label,
            human_bytes(req.system_bytes),
            human_bytes(req.last_user_bytes),
            req.last_user_hash.unwrap_or("-"),
        ),
    ));
    out.push_str(&wrap_row("headers", req.headers_present));
    out.push_str(&wrap_row("request_hash", req.request_hash));
    out.push_str(&format!("{}", "└─".dimmed()));
    info!(target: CONSOLE_TARGET, "{out}");
}

/// Convenience wrapper: take a fully-parsed [`ModelResponse`] plus a
/// wall-clock latency and emit the matching response block. Shared
/// between [`crate::AnthropicProvider::complete`] and the streaming
/// finalizer in `aura-agent::agent_loop::streaming`.
///
/// All callers in the harness target the LLM proxy, so the
/// destination is hard-coded to `"aura-network"` here. If a future
/// caller targets a different service tier, lift this to an explicit
/// parameter.
pub fn emit_response_block(
    response: &ModelResponse,
    elapsed_ms: u64,
    status_code: u16,
    status_text: &str,
) {
    let stop_label = format!("{:?}", response.stop_reason);
    anthropic_response_block(&AnthropicResponseView {
        status_code,
        status_text,
        stop_reason: &stop_label,
        elapsed_ms,
        input_tokens: response.usage.input_tokens,
        output_tokens: response.usage.output_tokens,
        request_id: response.trace.provider_request_id.as_deref(),
        destination: "aura-network",
    });
}

/// Render the response half of an Anthropic call symmetric to
/// [`anthropic_request_block`].
pub fn anthropic_response_block(resp: &AnthropicResponseView<'_>) {
    let mut out = String::new();
    let header_text = format!("← {} {}", resp.status_code, resp.status_text);
    let header = match resp.status_code {
        200..=299 => header_text.green().bold(),
        300..=399 => header_text.yellow().bold(),
        _ => header_text.red().bold(),
    };
    let tag = destination_tag(resp.destination, None);
    out.push_str(&format!("{} {header}  {tag}\n", "┌─".dimmed()));
    out.push_str(&wrap_row(
        "stop",
        &format!(
            "{:<14} elapsed {:>7}",
            resp.stop_reason,
            human_duration_ms(resp.elapsed_ms)
        ),
    ));
    out.push_str(&wrap_row(
        "tokens",
        &format!("in {} / out {}", resp.input_tokens, resp.output_tokens),
    ));
    if let Some(req_id) = resp.request_id {
        out.push_str(&wrap_row("request_id", req_id));
    }
    out.push_str(&format!("{}", "└─".dimmed()));
    info!(target: CONSOLE_TARGET, "{out}");
}

/// View into a blocked / failed `/v1/messages` round-trip. `status_code`
/// is `None` for failures that never produced an HTTP response (transport
/// timeout, DNS failure, body-serialize bug); for those the renderer
/// substitutes a `← transport failed` header.
///
/// Distinct from [`AnthropicResponseView`] so failure-only rows
/// (`class`, `request_id`, `retry_after`, `body`) do not have to fight
/// success-only rows (`tokens`) for column space.
pub struct AnthropicFailureView<'a> {
    pub status_code: Option<u16>,
    pub status_text: &'a str,
    /// Stable short class label: `cloudflare_block` / `rate_limited_429`
    /// / `upstream_5xx` / `insufficient_credits` / `transport` / `parse`
    /// / `sse_transport` / `other`. Mirrors the strings emitted by
    /// `retry_reason_for` plus a few non-HTTP buckets so an operator
    /// scanning the visual transcript can pivot to `retries.jsonl` by
    /// the same key.
    pub class: &'a str,
    pub elapsed_ms: u64,
    pub request_id: Option<&'a str>,
    pub retry_after_s: Option<u64>,
    /// Already-truncated body / error preview (≤140 chars recommended).
    /// Renderer collapses control chars + interior whitespace.
    pub body_preview: Option<&'a str>,
    pub destination: &'a str,
}

/// Planned next action the retry classifier picked for a failed
/// attempt. Rendered as a one-line `↻ retry …` / `→ fallback …` /
/// `✗ propagate …` continuation under the failure block so the
/// transcript shows what the harness will do about the block.
pub enum RetryDecisionView<'a> {
    Retry {
        attempt_that_failed: u32,
        max_retries: u32,
        sleep_ms: u64,
        body_cap_bytes: Option<usize>,
    },
    Fallback {
        next_model: &'a str,
    },
    Propagate {
        reason: &'a str,
    },
}

/// Render the failure half of an Anthropic call as a multi-line block.
/// Symmetric to [`anthropic_response_block`]: same destination tag,
/// same row layout, same target — operators reading the transcript
/// see one paired box per round-trip regardless of outcome.
pub fn anthropic_failure_block(view: &AnthropicFailureView<'_>) {
    let mut out = String::new();
    let header_text = match view.status_code {
        Some(code) => format!("← {} {}", code, view.status_text),
        None => format!("← {} ({})", view.status_text, view.class),
    };
    let header = header_text.red().bold();
    let tag = destination_tag(view.destination, None);
    out.push_str(&format!("{} {header}  {tag}\n", "┌─".dimmed()));
    out.push_str(&wrap_row(
        "class",
        &format!(
            "{:<14} elapsed {:>7}",
            view.class,
            human_duration_ms(view.elapsed_ms)
        ),
    ));
    if let Some(req_id) = view.request_id {
        out.push_str(&wrap_row("request_id", req_id));
    }
    if let Some(secs) = view.retry_after_s {
        out.push_str(&wrap_row("retry_after", &format!("{secs}s")));
    }
    if let Some(body) = view.body_preview {
        let collapsed = collapse_for_row(body, 140);
        if !collapsed.is_empty() {
            out.push_str(&wrap_row("body", &collapsed));
        }
    }
    out.push_str(&format!("{}", "└─".dimmed()));
    info!(target: CONSOLE_TARGET, "{out}");
}

/// Render the retry classifier's planned next action as a single
/// continuation line — sits between the failure block and the next
/// request block so the transcript shows what the harness will do
/// about the block without hunting through `warn!` lines.
pub fn anthropic_retry_decision_line(decision: &RetryDecisionView<'_>) {
    let body = match *decision {
        RetryDecisionView::Retry {
            attempt_that_failed,
            max_retries,
            sleep_ms,
            body_cap_bytes,
        } => {
            let cap =
                body_cap_bytes.map_or_else(String::new, |c| format!(" · cap {}", human_bytes(c)));
            // Attempt counts are 1-based for human readability:
            // `attempt_that_failed` is the 0-based index of the call
            // that just failed, so the *next* attempt is +2 of N.
            format!(
                "{}   {} retry {}/{} in {}{}",
                "│".dimmed(),
                "↻".yellow().bold(),
                attempt_that_failed.saturating_add(2),
                max_retries.saturating_add(1),
                human_duration_ms(sleep_ms),
                cap,
            )
        }
        RetryDecisionView::Fallback { next_model } => {
            format!(
                "{}   {} fallback to {next_model}",
                "│".dimmed(),
                "→".yellow().bold(),
            )
        }
        RetryDecisionView::Propagate { reason } => {
            format!(
                "{}   {} propagate ({reason})",
                "│".dimmed(),
                "✗".red().bold(),
            )
        }
    };
    info!(target: CONSOLE_TARGET, "{body}");
}

/// Derive a short host label from a base URL (e.g.
/// `"https://aura-router.onrender.com"` → `"aura-router.onrender.com"`).
/// Returns the input unchanged when no scheme prefix is found.
#[must_use]
pub fn extract_host(base_url: &str) -> &str {
    let after_scheme = base_url
        .strip_prefix("https://")
        .or_else(|| base_url.strip_prefix("http://"))
        .unwrap_or(base_url);
    after_scheme.split('/').next().unwrap_or(after_scheme)
}

#[cfg(test)]
mod tests;
