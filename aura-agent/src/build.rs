//! Build integration — auto-build checks and error annotation.

use crate::types::BuildBaseline;

/// Extract error signatures from build output.
///
/// Each signature is a normalized error block that can be compared
/// across builds to distinguish new from pre-existing errors.
pub fn extract_error_signatures(output: &str) -> Vec<String> {
    let mut signatures = Vec::new();
    let mut current_block = String::new();
    let mut in_error = false;

    for line in output.lines() {
        if line.starts_with("error[") || line.starts_with("error:") {
            if in_error && !current_block.is_empty() {
                signatures.push(normalize_error_block(&current_block));
                current_block.clear();
            }
            in_error = true;
            current_block.push_str(line);
            current_block.push('\n');
        } else if in_error {
            if line.is_empty() || line.starts_with("warning") {
                signatures.push(normalize_error_block(&current_block));
                current_block.clear();
                in_error = false;
            } else {
                current_block.push_str(line);
                current_block.push('\n');
            }
        }
    }

    if in_error && !current_block.is_empty() {
        signatures.push(normalize_error_block(&current_block));
    }

    signatures
}

/// Normalize an error block for comparison by stripping help text and location hints.
fn normalize_error_block(block: &str) -> String {
    block
        .lines()
        .filter(|l| !l.trim_start().starts_with("help:") && !l.trim_start().starts_with("-->"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Annotate build output with NEW vs PRE-EXISTING labels.
pub fn annotate_build_output(output: &str, baseline: &BuildBaseline) -> String {
    if baseline.error_signatures.is_empty() {
        return output.to_string();
    }

    let current_sigs = extract_error_signatures(output);
    let mut annotated = output.to_string();

    for sig in &current_sigs {
        let label = if baseline.error_signatures.contains(sig) {
            "[PRE-EXISTING]"
        } else {
            "[NEW]"
        };
        if let Some(first_line) = sig.lines().next() {
            if let Some(trimmed) = first_line.get(..first_line.len().min(60)) {
                annotated = annotated.replacen(trimmed, &format!("{label} {trimmed}"), 1);
            }
        }
    }

    annotated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_signatures_parses_error_blocks() {
        let output =
            "error[E0308]: mismatched types\n  --> src/main.rs:42:15\n\nerror[E0599]: no method\n";
        let sigs = extract_error_signatures(output);
        assert_eq!(sigs.len(), 2);
    }

    #[test]
    fn test_annotate_no_baseline() {
        let output = "error: something";
        let baseline = BuildBaseline::default();
        let result = annotate_build_output(output, &baseline);
        assert_eq!(result, output);
    }
}
