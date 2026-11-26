//! Markdown rendering of an already-resolved [`super::ResolvedContext`]
//! into the iteration-0 enrichment block.
//!
//! Pure: no IO, no async, no `WorkspaceReader`. Honours
//! [`super::ResolveCaps::max_block_chars`] by progressively dropping
//! the lowest-priority file-head bodies (the path/symbol lists are
//! never truncated mid-token).

use std::fmt::Write;

use super::types::{ResolvedContext, ResolvedPath, ResolvedSymbol};

/// Render `resolved` as a markdown block. Returns the empty string
/// when [`ResolvedContext::is_empty`] is true so callers can splice
/// the result unconditionally.
#[must_use]
pub fn into_block(resolved: &ResolvedContext) -> String {
    if resolved.is_empty() {
        return String::new();
    }
    let max_chars = resolved.max_block_chars;
    let mut path_bodies_kept: Vec<bool> = vec![true; resolved.paths.len()];
    loop {
        let rendered = render_block(
            &resolved.paths,
            &resolved.symbols,
            &path_bodies_kept,
            resolved.module_note.as_deref(),
        );
        if rendered.len() <= max_chars || max_chars == 0 {
            return rendered;
        }
        let Some(drop_idx) = path_bodies_kept
            .iter()
            .enumerate()
            .rev()
            .find(|(_, kept)| **kept)
            .map(|(i, _)| i)
        else {
            return rendered;
        };
        path_bodies_kept[drop_idx] = false;
    }
}

/// Public, low-level renderer. Most callers should go through
/// [`ResolvedContext::into_block`] which handles the
/// `max_block_chars` enforcement loop.
#[must_use]
pub fn render_block(
    paths: &[ResolvedPath],
    symbols: &[ResolvedSymbol],
    path_bodies_kept: &[bool],
    module_note: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str("## Pre-resolved context (from task description)\n\n");

    if let Some(note) = module_note {
        let _ = writeln!(out, "{note}\n");
    }

    if !paths.is_empty() {
        render_path_list(&mut out, paths, path_bodies_kept);
        render_path_heads(&mut out, paths, path_bodies_kept);
    }

    if !symbols.is_empty() {
        render_symbols(&mut out, symbols);
    }

    out.push_str(
        "Use these as starting points; you do NOT need to re-list the \
         directory or re-grep for these symbols.\n",
    );
    out
}

fn is_body_kept(path_bodies_kept: &[bool], index: usize) -> bool {
    *path_bodies_kept.get(index).unwrap_or(&false)
}

fn render_path_list(out: &mut String, paths: &[ResolvedPath], path_bodies_kept: &[bool]) {
    out.push_str("Files mentioned in the task that exist in the workspace:\n");
    for (i, p) in paths.iter().enumerate() {
        if is_body_kept(path_bodies_kept, i) && p.head.is_some() {
            let _ = writeln!(
                out,
                "- `{}` (file head, lines 1-{} below)",
                p.path, p.head_line_count
            );
        } else {
            let _ = writeln!(out, "- `{}`", p.path);
        }
    }
    out.push('\n');
}

fn render_path_heads(out: &mut String, paths: &[ResolvedPath], path_bodies_kept: &[bool]) {
    for (i, p) in paths.iter().enumerate() {
        if !is_body_kept(path_bodies_kept, i) {
            continue;
        }
        let Some(head) = &p.head else {
            continue;
        };
        let _ = writeln!(out, "### {} (lines 1-{})", p.path, p.head_line_count);
        out.push_str("```rust\n");
        out.push_str(head);
        if !head.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n\n");
    }
}

fn render_symbols(out: &mut String, symbols: &[ResolvedSymbol]) {
    out.push_str("Symbols referenced in the task:\n");
    for s in symbols {
        if let Some(first) = s.hits.first() {
            let _ = writeln!(out, "- `{}` -> {}:{}", s.symbol, first.path, first.line);
            for extra in s.hits.iter().skip(1) {
                let _ = writeln!(out, "  also: {}:{}", extra.path, extra.line);
            }
        }
    }
    out.push('\n');
}
