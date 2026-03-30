//! Error signature normalization for detecting stagnant fix loops.
//!
//! Strips line numbers, column numbers, and file paths from compiler stderr so
//! that the same class of error compares as equal even when line numbers shift
//! between fix attempts.

use std::collections::HashSet;

/// Produce a normalized "signature" from compiler stderr by stripping
/// line/column numbers and file paths.
pub fn normalize_error_signature(stderr: &str) -> String {
    let mut signature_lines: Vec<String> = Vec::new();
    for line in stderr.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("For more information")
            || trimmed.starts_with("help:")
        {
            continue;
        }
        if trimmed.starts_with("-->") {
            signature_lines.push("-->LOCATION".into());
            continue;
        }
        if trimmed.chars().next().is_some_and(|c| c.is_ascii_digit()) && trimmed.contains('|') {
            continue;
        }
        if trimmed
            .chars()
            .all(|c| c == '^' || c == '-' || c == ' ' || c == '~' || c == '+')
        {
            continue;
        }
        let normalized = normalize_line_col_refs(trimmed);
        if !normalized.is_empty() {
            signature_lines.push(normalized);
        }
    }
    signature_lines.sort();
    signature_lines.dedup();
    signature_lines.join("\n")
}

fn normalize_line_col_refs(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ':' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
            result.push(':');
            result.push('N');
            i += 1;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

/// Split compiler stderr into individual error blocks and normalize each one
/// independently, returning a set of unique error signatures.
pub fn parse_individual_error_signatures(stderr: &str) -> HashSet<String> {
    let mut signatures = HashSet::new();
    let mut current_block = String::new();
    let mut in_error_block = false;
    for line in stderr.lines() {
        if line.starts_with("error[") || line.starts_with("error:") {
            if in_error_block && !current_block.is_empty() {
                let sig = normalize_error_signature(&current_block);
                if !sig.is_empty() {
                    signatures.insert(sig);
                }
                current_block.clear();
            }
            in_error_block = true;
        }
        if in_error_block {
            current_block.push_str(line);
            current_block.push('\n');
        }
    }
    if in_error_block && !current_block.is_empty() {
        let sig = normalize_error_signature(&current_block);
        if !sig.is_empty() {
            signatures.insert(sig);
        }
    }
    signatures
}
