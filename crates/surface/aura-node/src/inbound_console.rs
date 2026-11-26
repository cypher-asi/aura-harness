//! Visual block helpers for the inbound HTTP / WebSocket transcript.
//!
//! Symmetric to [`aura_agent::console`] / [`aura_model::console`]:
//! emits multi-line "blocks" (`┌─ → GET /api/files` / `┌─ ← 401
//! unauthorized`) and one-line continuation rows under the
//! dedicated `"aura::console"` tracing target so the custom event
//! formatter in [`crate::console_format`] renders them verbatim.
//!
//! ## Why a dedicated module
//!
//! The outbound LLM-call transcript already pairs every `→ POST` with
//! either `← 200 OK` (success) or `← <status> <reason>` (failure).
//! On the inbound side, multiple rejection paths historically returned
//! a `StatusCode::into_response()` with no log at all (auth-middleware
//! 401, governor 429, body-limit 413, timeout 408, WS slot full,
//! inbound-frame parse errors). This module gives those rejections a
//! shared visual surface so an operator
//! scanning a single log file sees the same `→ in` / `← out` pairing
//! for inbound traffic that already exists for outbound calls.
//!
//! ## Layout & colors
//!
//! Mirrors the outbound modules: row helper computes column widths,
//! borders / labels are dimmed, header carries a status-family color,
//! `colored` auto-disables when stdout is not a TTY and honors
//! `NO_COLOR`. The `wrap_row` / `human_*` / `collapse_for_row` helpers
//! are shared with the outbound modules via
//! [`aura_model::console::layout`].

use std::net::SocketAddr;

use aura_model::console::layout::{collapse_for_row, human_bytes, human_duration_ms, wrap_row};
use colored::Colorize;
use tracing::info;

/// Tracing target the runtime's custom formatter renders verbatim.
/// Must stay in sync with [`crate::console_format::CONSOLE_TARGETS`];
/// re-exported from `aura-model` so all three transcripts share it.
pub use aura_model::console::CONSOLE_TARGET;

/// View into an inbound HTTP request — populated by the failure
/// observer middleware right before it emits the paired `→`/`←`
/// blocks. The summary block is only rendered on the failure path
/// (success requests stay quiet to keep transcript noise low).
pub struct InboundRequestView<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub peer: Option<SocketAddr>,
    /// Inbound `Content-Length` if the request reported one. `None`
    /// when the request had no body or used chunked transfer.
    pub body_bytes: Option<usize>,
}

/// View into the rejected inbound response. `reason` is a short stable
/// label (`"unauthorized"`, `"rate_limited"`, `"body_too_large"`,
/// `"timeout"`, `"upstream_5xx"`, `"error_<code>"`) the middleware
/// derives from the response status; renderer puts it in the header
/// and as a `class` row so an operator can scan either column.
pub struct InboundFailureView<'a> {
    pub status_code: u16,
    pub status_text: &'a str,
    pub reason: &'a str,
    pub elapsed_ms: u64,
    pub peer: Option<SocketAddr>,
    /// Optional response-body preview for handler-emitted error JSON
    /// (e.g. `ApiError::bad_request("...")`). Caller should already
    /// have collapsed control chars / clipped to a sane length.
    pub body_preview: Option<&'a str>,
}

/// Render the request half of an inbound rejection as a multi-line
/// block. Only emitted when the response was non-2xx.
pub fn inbound_request_summary_block(view: &InboundRequestView<'_>) {
    let mut out = String::new();
    let header = format!("→ {} {}", view.method, view.path).cyan().bold();
    let tag = peer_tag(view.peer);
    out.push_str(&format!("{} {header}  {tag}\n", "┌─".dimmed()));
    if let Some(bytes) = view.body_bytes {
        out.push_str(&wrap_row("body", &human_bytes(bytes)));
    }
    out.push_str(&format!("{}", "└─".dimmed()));
    info!(target: CONSOLE_TARGET, "{out}");
}

/// Render the response half of an inbound rejection. Header is
/// red-bold so a 401 / 429 / 5xx jumps out of the transcript.
pub fn inbound_failure_block(view: &InboundFailureView<'_>) {
    let mut out = String::new();
    let header = format!("← {} {}", view.status_code, view.reason)
        .red()
        .bold();
    let tag = peer_tag(view.peer);
    out.push_str(&format!("{} {header}  {tag}\n", "┌─".dimmed()));
    out.push_str(&wrap_row(
        "status",
        &format!(
            "{:<14} elapsed {:>7}",
            view.status_text,
            human_duration_ms(view.elapsed_ms)
        ),
    ));
    if let Some(body) = view.body_preview {
        let collapsed = collapse_for_row(body, 140);
        if !collapsed.is_empty() {
            out.push_str(&wrap_row("body", &collapsed));
        }
    }
    out.push_str(&format!("{}", "└─".dimmed()));
    info!(target: CONSOLE_TARGET, "{out}");
}

/// Render a single-line rejection notice for paths that don't have a
/// natural HTTP request/response shape — WebSocket upgrade refusals,
/// inbound-frame parse errors, malformed control messages, etc.
///
/// `scope` is a short tag like `"upgrade"`, `"upgrade.automaton"`,
/// `"framing"`. `reason` is a stable label that mirrors the error
/// `code` shipped to the client (so server / client log correlation
/// is trivial). `detail` is an optional free-form preview.
pub fn ws_rejection_line(scope: &str, reason: &str, detail: Option<&str>) {
    let detail_part = match detail {
        Some(d) if !d.is_empty() => format!(": {}", collapse_for_row(d, 140)),
        _ => String::new(),
    };
    let body = format!(
        "{} {} ws.{scope} {reason}{detail_part}",
        "│".dimmed(),
        "⊘".red().bold(),
    );
    info!(target: CONSOLE_TARGET, "{body}");
}

/// Map an HTTP status code to the short stable label rendered in
/// the failure block header. Lookup table kept here so the inbound
/// middleware and the WS-rejection paths share one vocabulary.
#[must_use]
pub fn reason_for_status(status: u16) -> &'static str {
    match status {
        400 => "bad_request",
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        405 => "method_not_allowed",
        408 => "timeout",
        409 => "conflict",
        413 => "body_too_large",
        415 => "unsupported_media_type",
        422 => "unprocessable",
        429 => "rate_limited",
        500 => "server_error",
        501 => "not_implemented",
        502..=504 => "upstream_5xx",
        _ if (400..500).contains(&status) => "client_error",
        _ if (500..600).contains(&status) => "server_error",
        _ => "unknown",
    }
}

// ----------------------------------------------------------------------
// Formatting primitives — row / value helpers come from
// `aura_model::console::layout`; only the peer tag is inbound-specific.
// ----------------------------------------------------------------------

fn peer_tag(peer: Option<SocketAddr>) -> String {
    let body = match peer {
        Some(addr) => format!("[{addr}]"),
        None => String::from("[-]"),
    };
    body.cyan().dimmed().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loopback() -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 12345))
    }

    #[test]
    fn reason_for_status_known_codes() {
        assert_eq!(reason_for_status(401), "unauthorized");
        assert_eq!(reason_for_status(403), "forbidden");
        assert_eq!(reason_for_status(404), "not_found");
        assert_eq!(reason_for_status(408), "timeout");
        assert_eq!(reason_for_status(413), "body_too_large");
        assert_eq!(reason_for_status(429), "rate_limited");
        assert_eq!(reason_for_status(502), "upstream_5xx");
        assert_eq!(reason_for_status(418), "client_error");
        assert_eq!(reason_for_status(599), "server_error");
    }

    #[test]
    fn inbound_request_summary_block_renders_without_panic() {
        inbound_request_summary_block(&InboundRequestView {
            method: "GET",
            path: "/api/files",
            peer: Some(loopback()),
            body_bytes: None,
        });
        inbound_request_summary_block(&InboundRequestView {
            method: "POST",
            path: "/tx",
            peer: Some(loopback()),
            body_bytes: Some(2048),
        });
    }

    #[test]
    fn inbound_failure_block_renders_each_status_family() {
        inbound_failure_block(&InboundFailureView {
            status_code: 401,
            status_text: "Unauthorized",
            reason: "unauthorized",
            elapsed_ms: 4,
            peer: Some(loopback()),
            body_preview: None,
        });
        inbound_failure_block(&InboundFailureView {
            status_code: 429,
            status_text: "Too Many Requests",
            reason: "rate_limited",
            elapsed_ms: 1,
            peer: Some(loopback()),
            body_preview: None,
        });
        inbound_failure_block(&InboundFailureView {
            status_code: 413,
            status_text: "Payload Too Large",
            reason: "body_too_large",
            elapsed_ms: 1,
            peer: Some(loopback()),
            body_preview: Some("body too large"),
        });
        inbound_failure_block(&InboundFailureView {
            status_code: 408,
            status_text: "Request Timeout",
            reason: "timeout",
            elapsed_ms: 30_000,
            peer: Some(loopback()),
            body_preview: None,
        });
        inbound_failure_block(&InboundFailureView {
            status_code: 502,
            status_text: "Bad Gateway",
            reason: "upstream_5xx",
            elapsed_ms: 87,
            peer: None,
            body_preview: None,
        });
    }

    #[test]
    fn ws_rejection_line_renders_each_scope() {
        ws_rejection_line("upgrade", "slot_full", Some("cap=128"));
        ws_rejection_line(
            "upgrade.automaton",
            "unauthorized",
            Some("automaton_id=foo"),
        );
        ws_rejection_line(
            "framing",
            "parse_error",
            Some("expected `,` or `}` at line 1 column 4"),
        );
        ws_rejection_line("framing", "transport_error", None);
        ws_rejection_line("framing", "not_initialized", None);
    }
}
