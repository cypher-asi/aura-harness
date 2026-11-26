//! Spec domain tool handlers.

use serde_json::{json, Value};
use tracing::debug;

use super::api::{DomainApi, SpecDescriptor};
use super::helpers::{domain_err, domain_ok, require_str, str_field};

/// Maximum bytes of `markdown_contents` returned by `get_spec` before the body
/// is truncated and a marker appended. Mirrors the `fs_read` cap so a single
/// tool result can never exceed the upstream proxy's per-message envelope.
const MAX_SPEC_MARKDOWN_BYTES: usize = 64 * 1024;

/// Find the largest byte index `<= max_bytes` that lies on a UTF-8 char
/// boundary of `s`. Mirrors the pattern used by `fs_read` for the
/// line-sliced render path.
fn truncate_on_utf8_boundary(s: &str, max_bytes: usize) -> usize {
    if s.len() <= max_bytes {
        return s.len();
    }
    let mut idx = max_bytes;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Standard marker appended to a truncated `markdown_contents` so the LLM
/// gets a consistent, machine-readable hint about how to recover the missing
/// bytes (or to stop re-reading the full body each turn).
fn spec_truncation_marker(dropped: usize, total: usize) -> String {
    format!(
        "\n... [truncated {dropped} of {total} bytes; fetch the spec via a slice/paginated tool or read fewer fields if you need the full body.]"
    )
}

/// Build the LLM-facing JSON envelope for a single spec, applying the 64 KB
/// cap on `markdown_contents` and surfacing `truncated_markdown` /
/// `total_markdown_bytes` siblings when the body is truncated. All other
/// fields are passed through unchanged.
fn build_spec_json(s: &SpecDescriptor) -> Value {
    let total = s.content.len();
    let kept = truncate_on_utf8_boundary(&s.content, MAX_SPEC_MARKDOWN_BYTES);
    let truncated = kept < total;
    let body = if truncated {
        let mut out = String::with_capacity(kept + 128);
        out.push_str(&s.content[..kept]);
        out.push_str(&spec_truncation_marker(total - kept, total));
        out
    } else {
        s.content.clone()
    };
    let mut spec = json!({
        "id": s.id,
        "project_id": s.project_id,
        "title": s.title,
        "markdown_contents": body,
        "order": s.order,
        "parent_id": s.parent_id,
    });
    if let Some(hash) = &s.content_hash {
        // Surface the optimistic-concurrency token so the LLM can pass it
        // back as `if_match` on the next edit.
        spec["content_hash"] = json!(hash);
    }
    if truncated {
        spec["truncated_markdown"] = json!(true);
        spec["total_markdown_bytes"] = json!(total);
    }
    spec
}

pub async fn list_specs(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!(project_id, "domain_tools: list_specs");
    let jwt = str_field(input, "jwt");
    match api.list_specs(project_id, jwt.as_deref()).await {
        Ok(specs) => {
            let summaries: Vec<Value> = specs
                .iter()
                .map(|s| {
                    json!({
                        "spec_id": s.id,
                        "title": s.title,
                        "order": s.order,
                        "markdown_bytes": s.content.len(),
                    })
                })
                .collect();
            domain_ok(json!({ "specs": summaries }))
        }
        Err(e) => domain_err(e),
    }
}

pub async fn get_spec(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!(project_id, "domain_tools: get_spec");
    let spec_id = match require_str(input, "spec_id") {
        Ok(id) => id,
        Err(e) => return domain_err(&e),
    };
    let jwt = str_field(input, "jwt");
    match api.get_spec(&spec_id, jwt.as_deref()).await {
        Ok(s) => domain_ok(json!({ "spec": build_spec_json(&s) })),
        Err(e) => domain_err(e),
    }
}

pub async fn create_spec(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!(project_id, "domain_tools: create_spec");
    let title = str_field(input, "title").unwrap_or_default();
    let content = str_field(input, "markdown_contents")
        .or_else(|| str_field(input, "content"))
        .unwrap_or_default();
    let jwt = str_field(input, "jwt");

    // Auto-derive orderIndex from existing spec count so specs are
    // numbered in creation order without the caller needing to track it.
    let order = match api.list_specs(project_id, jwt.as_deref()).await {
        #[allow(clippy::cast_possible_truncation)]
        Ok(specs) => specs.len() as u32,
        Err(_) => 0,
    };

    match api
        .create_spec(project_id, &title, &content, order, jwt.as_deref())
        .await
    {
        Ok(s) => domain_ok(json!({ "spec": s })),
        Err(e) => domain_err(e),
    }
}

pub async fn update_spec(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: update_spec");
    let spec_id = match require_str(input, "spec_id") {
        Ok(id) => id,
        Err(e) => return domain_err(&e),
    };
    let title = str_field(input, "title");
    let content = str_field(input, "markdown_contents").or_else(|| str_field(input, "content"));
    let if_match = str_field(input, "if_match");
    let jwt = str_field(input, "jwt");

    match api
        .update_spec(
            &spec_id,
            title.as_deref(),
            content.as_deref(),
            if_match.as_deref(),
            jwt.as_deref(),
        )
        .await
    {
        Ok(s) => domain_ok(json!({ "spec": s })),
        Err(e) => domain_err(e),
    }
}

pub async fn update_spec_section(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: update_spec_section");
    let spec_id = match require_str(input, "spec_id") {
        Ok(id) => id,
        Err(e) => return domain_err(&e),
    };
    let section_heading = match require_str(input, "section_heading") {
        Ok(h) => h,
        Err(e) => return domain_err(&e),
    };
    let new_body = match require_str(input, "new_body") {
        Ok(b) => b,
        Err(e) => return domain_err(&e),
    };
    let if_match = str_field(input, "if_match");
    let jwt = str_field(input, "jwt");

    match api
        .update_spec_section(
            &spec_id,
            &section_heading,
            &new_body,
            if_match.as_deref(),
            jwt.as_deref(),
        )
        .await
    {
        Ok(s) => domain_ok(json!({ "spec": s })),
        Err(e) => domain_err(e),
    }
}

pub async fn append_to_spec(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: append_to_spec");
    let spec_id = match require_str(input, "spec_id") {
        Ok(id) => id,
        Err(e) => return domain_err(&e),
    };
    let markdown = match require_str(input, "markdown") {
        Ok(m) => m,
        Err(e) => return domain_err(&e),
    };
    let if_match = str_field(input, "if_match");
    let jwt = str_field(input, "jwt");

    match api
        .append_to_spec(&spec_id, &markdown, if_match.as_deref(), jwt.as_deref())
        .await
    {
        Ok(s) => domain_ok(json!({ "spec": s })),
        Err(e) => domain_err(e),
    }
}

pub async fn delete_spec(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: delete_spec");
    let spec_id = match require_str(input, "spec_id") {
        Ok(id) => id,
        Err(e) => return domain_err(&e),
    };
    let jwt = str_field(input, "jwt");
    match api.delete_spec(&spec_id, jwt.as_deref()).await {
        Ok(()) => domain_ok(json!({ "deleted": spec_id })),
        Err(e) => domain_err(e),
    }
}

#[cfg(test)]
mod tests;
