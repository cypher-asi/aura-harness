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

/// Classify build errors into categories so the fix prompt can include
/// targeted guidance instead of generic "try a different approach."
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorCategory {
    RustStringLiteral,
    RustMissingModule,
    RustMissingMethod,
    RustTypeError,
    RustBorrowCheck,
    RustStructFieldMismatch,
    RustMissingImport,
    NpmDependency,
    NpmTypeScript,
    GenericSyntax,
    Unknown,
}

pub fn classify_build_errors(stderr: &str) -> Vec<ErrorCategory> {
    let mut categories = Vec::new();

    push_rust_syntax_categories(stderr, &mut categories);
    push_rust_semantic_categories(stderr, &mut categories);

    if stderr.contains("Cannot find module") || stderr.contains("ENOENT") {
        categories.push(ErrorCategory::NpmDependency);
    }

    if stderr.contains("TS2304") || stderr.contains("TS2345") || stderr.contains("TS2322") {
        categories.push(ErrorCategory::NpmTypeScript);
    }

    if categories.is_empty()
        && (stderr.contains("expected")
            || stderr.contains("syntax error")
            || stderr.contains("parse error"))
    {
        categories.push(ErrorCategory::GenericSyntax);
    }

    if categories.is_empty() {
        categories.push(ErrorCategory::Unknown);
    }
    categories
}

/// Rust token, module, import and method-resolution categories.
fn push_rust_syntax_categories(stderr: &str, categories: &mut Vec<ErrorCategory>) {
    let rust_string_patterns = [
        "unknown start of token",
        "prefix `",
        "unknown prefix",
        "Unicode character",
        "looks like",
        "but it is not",
    ];
    if rust_string_patterns.iter().any(|p| stderr.contains(p)) {
        categories.push(ErrorCategory::RustStringLiteral);
    }

    if stderr.contains("file not found for module") || stderr.contains("E0583") {
        categories.push(ErrorCategory::RustMissingModule);
    }

    if stderr.contains("cannot find")
        || stderr.contains("E0425")
        || stderr.contains("E0433")
        || stderr.contains("not found in this scope")
        || stderr.contains("use of undeclared")
    {
        categories.push(ErrorCategory::RustMissingImport);
    }

    if stderr.contains("no method named") || stderr.contains("E0599") {
        categories.push(ErrorCategory::RustMissingMethod);
    }
}

/// Rust struct-field, type and borrow-check categories.
fn push_rust_semantic_categories(stderr: &str, categories: &mut Vec<ErrorCategory>) {
    if stderr.contains("missing field")
        || stderr.contains("E0063")
        || stderr.contains("has no field named")
        || stderr.contains("E0560")
    {
        categories.push(ErrorCategory::RustStructFieldMismatch);
    }

    if stderr.contains("the trait") && stderr.contains("is not implemented")
        || stderr.contains("E0277")
        || stderr.contains("type annotations needed")
        || stderr.contains("E0283")
    {
        categories.push(ErrorCategory::RustTypeError);
    }

    if stderr.contains("cannot borrow") || stderr.contains("E0502") || stderr.contains("E0505") {
        categories.push(ErrorCategory::RustBorrowCheck);
    }
}

pub fn error_category_guidance(categories: &[ErrorCategory]) -> String {
    let mut guidance = String::new();
    for cat in categories {
        let advice: &str = match cat {
            ErrorCategory::RustStringLiteral => RUST_STRING_LITERAL_ADVICE,
            ErrorCategory::RustMissingModule => RUST_MISSING_MODULE_ADVICE,
            ErrorCategory::RustMissingMethod => RUST_MISSING_METHOD_ADVICE,
            ErrorCategory::RustTypeError => RUST_TYPE_ERROR_ADVICE,
            ErrorCategory::RustBorrowCheck => RUST_BORROW_CHECK_ADVICE,
            ErrorCategory::RustStructFieldMismatch => RUST_STRUCT_FIELD_MISMATCH_ADVICE,
            ErrorCategory::RustMissingImport => RUST_MISSING_IMPORT_ADVICE,
            ErrorCategory::NpmDependency => NPM_DEPENDENCY_ADVICE,
            ErrorCategory::NpmTypeScript => NPM_TYPE_SCRIPT_ADVICE,
            ErrorCategory::GenericSyntax => GENERIC_SYNTAX_ADVICE,
            ErrorCategory::Unknown => "",
        };
        if !advice.is_empty() {
            guidance.push_str(advice);
            guidance.push_str("\n\n");
        }
    }
    guidance
}

const RUST_STRING_LITERAL_ADVICE: &str = concat!(
    "DIAGNOSIS: Rust string literal / token errors detected.\n",
    "ROOT CAUSE: This almost always means JSON or text with special characters ",
    "was placed directly in Rust source code without proper string escaping.\n",
    "MANDATORY FIX:\n",
    "- For test fixtures or multi-line strings containing JSON, quotes, backslashes, ",
    "or special chars: use Rust RAW STRING LITERALS (r followed by # then quote to open, ",
    "quote then # to close; add more # symbols if the content itself contains that pattern).\n",
    "- For programmatic JSON construction: use serde_json::json!() macro instead of string literals.\n",
    "- NEVER put literal backslash-n (two characters) inside a Rust string to represent a newline; ",
    "use actual newlines inside raw strings, or proper escape sequences inside regular strings.\n",
    "- NEVER use non-ASCII characters (em dashes, smart quotes, etc.) in Rust string literals; ",
    "replace with ASCII equivalents.\n",
    "- Check ALL string literals in the file, not just the ones the compiler flagged -- ",
    "the same mistake is likely repeated.",
);

const RUST_MISSING_MODULE_ADVICE: &str = concat!(
    "DIAGNOSIS: Missing Rust module file.\n",
    "FIX: If mod.rs or lib.rs declares `pub mod foo;`, the file `foo.rs` ",
    "(or `foo/mod.rs`) MUST exist. Either create the file or remove the module declaration.",
);

const RUST_MISSING_METHOD_ADVICE: &str = concat!(
    "DIAGNOSIS: Method not found on type.\n",
    "FIX: Check the actual public API of the type (read its source file). ",
    "Do not invent methods. If the method does not exist, either implement it ",
    "or use an existing method that provides the same functionality.",
);

const RUST_TYPE_ERROR_ADVICE: &str = concat!(
    "DIAGNOSIS: Type mismatch or missing trait implementation.\n",
    "FIX: Read the function signatures carefully. Check generic type parameters. ",
    "Provide explicit type annotations where the compiler asks for them. ",
    "Do not use `[u8]` where `Vec<u8>` or `&[u8]` is needed.",
);

const RUST_BORROW_CHECK_ADVICE: &str = concat!(
    "DIAGNOSIS: Borrow checker violation.\n",
    "FIX: Check ownership and lifetimes. Consider cloning, using references, ",
    "or restructuring to avoid simultaneous mutable/immutable borrows.",
);

const RUST_STRUCT_FIELD_MISMATCH_ADVICE: &str = concat!(
    "DIAGNOSIS: Struct field mismatch -- fields were added, removed, or renamed.\n",
    "FIX: Read the actual struct definition in the 'Actual API Reference' section below. ",
    "Update every initializer and field access to match the current struct fields exactly. ",
    "Add any new required fields (use Default/None for Option types), remove fields that ",
    "no longer exist, and rename fields that were renamed.\n",
);

const RUST_MISSING_IMPORT_ADVICE: &str = concat!(
    "DIAGNOSIS: Missing import or undeclared type/value (E0425/E0433).\n",
    "FIX: Add the missing `use` statement. The compiler help message usually shows the exact ",
    "import path. Common cases:\n",
    "- Standard library types: `use std::path::{Path, PathBuf};`, `use std::collections::HashMap;`\n",
    "- Crate-local items: `use crate::module::Item;`\n",
    "- Items in test modules: if a type/function exists in another module's `#[cfg(test)]` block, ",
    "it is NOT accessible from outside. Use the public API or duplicate the helper.\n",
    "- If the item genuinely doesn't exist on the type (method not found), check whether tests ",
    "are calling functions from the wrong module -- e.g. `Struct::func()` when `func` is a ",
    "free function in `crate::other_module`.",
);

const NPM_DEPENDENCY_ADVICE: &str = concat!(
    "DIAGNOSIS: Missing npm package or module.\n",
    "FIX: Ensure the dependency exists in package.json and has been installed. ",
    "Check import paths for typos.",
);

const NPM_TYPE_SCRIPT_ADVICE: &str = concat!(
    "DIAGNOSIS: TypeScript type errors.\n",
    "FIX: Check that types align with the library's actual API. ",
    "Read type definitions if needed.",
);

const GENERIC_SYNTAX_ADVICE: &str = concat!(
    "DIAGNOSIS: Syntax error.\n",
    "FIX: Look at the exact line/column the compiler indicates. ",
    "Check for missing semicolons, unbalanced braces, or misplaced tokens.",
);

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

    #[test]
    fn classify_rust_string_literal() {
        let errors = classify_build_errors("error: unknown start of token \\u{201c}");
        assert!(errors.contains(&ErrorCategory::RustStringLiteral));
    }

    #[test]
    fn classify_rust_missing_module() {
        let errors = classify_build_errors("error[E0583]: file not found for module `foo`");
        assert!(errors.contains(&ErrorCategory::RustMissingModule));
    }

    #[test]
    fn classify_rust_borrow_check() {
        let errors = classify_build_errors("error[E0502]: cannot borrow `x` as mutable");
        assert!(errors.contains(&ErrorCategory::RustBorrowCheck));
    }

    #[test]
    fn classify_npm_dependency() {
        let errors = classify_build_errors("Error: Cannot find module 'express'");
        assert!(errors.contains(&ErrorCategory::NpmDependency));
    }

    #[test]
    fn classify_npm_typescript() {
        let errors = classify_build_errors("error TS2304: Cannot find name 'foo'");
        assert!(errors.contains(&ErrorCategory::NpmTypeScript));
    }

    #[test]
    fn classify_generic_syntax() {
        let errors = classify_build_errors("syntax error near unexpected token");
        assert!(errors.contains(&ErrorCategory::GenericSyntax));
    }

    #[test]
    fn classify_unknown_fallback() {
        let errors = classify_build_errors("something completely unknown happened");
        assert!(errors.contains(&ErrorCategory::Unknown));
    }

    #[test]
    fn classify_multiple_categories() {
        let stderr = "error[E0599]: no method named `foo`\nerror[E0502]: cannot borrow `x`";
        let errors = classify_build_errors(stderr);
        assert!(errors.contains(&ErrorCategory::RustMissingMethod));
        assert!(errors.contains(&ErrorCategory::RustBorrowCheck));
    }
}
