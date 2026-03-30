use std::collections::HashMap;
use std::path::Path;

use super::FileOpsError;

struct WorkspaceMetadata {
    members: Vec<String>,
    crate_names: HashMap<String, String>,
    crate_deps: HashMap<String, Vec<String>>,
    crate_docs: HashMap<String, String>,
}

fn parse_workspace_metadata(root: &Path) -> Option<WorkspaceMetadata> {
    let cargo_content = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let members = parse_workspace_members(&cargo_content);
    if members.is_empty() {
        return None;
    }

    let mut crate_names = HashMap::new();
    let mut crate_deps = HashMap::new();
    let mut crate_docs = HashMap::new();

    for member in &members {
        let content = match std::fs::read_to_string(root.join(member).join("Cargo.toml")) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let name = parse_package_name(&content).unwrap_or_else(|| member.clone());
        crate_names.insert(member.clone(), name);
        crate_deps.insert(member.clone(), parse_internal_deps(&content));
        let doc = read_crate_doc_comment(root, member);
        if !doc.is_empty() {
            crate_docs.insert(member.clone(), doc);
        }
    }
    Some(WorkspaceMetadata {
        members,
        crate_names,
        crate_deps,
        crate_docs,
    })
}

fn format_workspace_map(
    meta: &WorkspaceMetadata,
    name_to_path: &HashMap<String, String>,
) -> String {
    let mut output = format!("Workspace: {} crates\n", meta.members.len());
    for member in &meta.members {
        let name = meta
            .crate_names
            .get(member)
            .map(|s| s.as_str())
            .unwrap_or(member);
        let doc = meta
            .crate_docs
            .get(member)
            .map(|s| s.as_str())
            .unwrap_or("");
        let doc_suffix = if doc.is_empty() {
            String::new()
        } else {
            format!(" -- {doc}")
        };
        output.push_str(&format!("  {member} ({name}){doc_suffix}\n"));
        if let Some(deps) = meta.crate_deps.get(member) {
            let resolved: Vec<&str> = deps
                .iter()
                .filter(|d| name_to_path.contains_key(d.as_str()))
                .map(|d| d.as_str())
                .collect();
            if resolved.is_empty() {
                output.push_str("    deps: []\n");
            } else {
                output.push_str(&format!("    deps: [{}]\n", resolved.join(", ")));
            }
        }
    }
    output
}

/// Parse the root `Cargo.toml` for `[workspace].members`, resolve each
/// member's internal dependencies, and produce a compact structural summary
/// (~2K tokens) suitable for prompt injection.
pub fn generate_workspace_map(project_root: &str) -> Result<String, FileOpsError> {
    let root = Path::new(project_root);
    let meta = match parse_workspace_metadata(root) {
        Some(m) => m,
        None => return Ok(String::new()),
    };
    let name_to_path: HashMap<String, String> = meta
        .crate_names
        .iter()
        .map(|(p, n)| (n.clone(), p.clone()))
        .collect();
    Ok(format_workspace_map(&meta, &name_to_path))
}

pub(crate) fn parse_workspace_members(cargo_content: &str) -> Vec<String> {
    let mut members = Vec::new();
    let mut in_members = false;

    for line in cargo_content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("members") && trimmed.contains('[') {
            in_members = true;
            if trimmed.contains(']') {
                extract_quoted_strings(trimmed, &mut members);
                break;
            }
            extract_quoted_strings(trimmed, &mut members);
            continue;
        }
        if in_members {
            if trimmed.contains(']') {
                extract_quoted_strings(trimmed, &mut members);
                break;
            }
            extract_quoted_strings(trimmed, &mut members);
        }
    }
    members
}

fn extract_quoted_strings(line: &str, out: &mut Vec<String>) {
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        rest = &rest[start + 1..];
        if let Some(end) = rest.find('"') {
            out.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
}

pub(crate) fn parse_package_name(cargo_content: &str) -> Option<String> {
    let mut in_package = false;
    for line in cargo_content.lines() {
        let trimmed = line.trim();
        if trimmed == "[package]" {
            in_package = true;
            continue;
        }
        if trimmed.starts_with('[') && trimmed != "[package]" {
            if in_package {
                break;
            }
            continue;
        }
        if in_package && trimmed.starts_with("name") {
            if let Some(val) = trimmed.split('=').nth(1) {
                let val = val.trim().trim_matches('"').trim_matches('\'');
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Extract workspace-internal dependency names from a crate's Cargo.toml.
/// We detect path dependencies (those with `path = "..."`) and return
/// the package name (from `package = "..."` override or the dep key itself).
pub(crate) fn parse_internal_deps(cargo_content: &str) -> Vec<String> {
    let mut deps = Vec::new();
    let mut in_deps = false;
    let mut in_inline_table = false;
    let mut current_dep_name = String::new();

    for line in cargo_content.lines() {
        let trimmed = line.trim();

        if trimmed == "[dependencies]" || trimmed == "[dev-dependencies]" {
            in_deps = trimmed == "[dependencies]";
            in_inline_table = false;
            continue;
        }
        if trimmed.starts_with('[') {
            if trimmed.starts_with("[dependencies.") {
                let dep_name = trimmed
                    .trim_start_matches("[dependencies.")
                    .trim_end_matches(']');
                current_dep_name = dep_name.to_string();
                in_inline_table = true;
                in_deps = false;
                continue;
            }
            in_deps = false;
            in_inline_table = false;
            continue;
        }

        if in_inline_table {
            if trimmed.starts_with("path") {
                deps.push(current_dep_name.clone());
                in_inline_table = false;
            }
            continue;
        }

        if in_deps && trimmed.contains("path") && trimmed.contains('=') {
            let dep_name = trimmed.split('=').next().unwrap_or("").trim();
            if !dep_name.is_empty() {
                deps.push(dep_name.to_string());
            }
        }
    }
    deps
}

/// Read the first 5 lines of a crate's lib.rs or main.rs to extract
/// any `//!` module-level doc comment as a short description.
fn read_crate_doc_comment(project_root: &Path, member: &str) -> String {
    let src_dir = project_root.join(member).join("src");
    let entry_file = if src_dir.join("lib.rs").exists() {
        src_dir.join("lib.rs")
    } else if src_dir.join("main.rs").exists() {
        src_dir.join("main.rs")
    } else {
        return String::new();
    };

    let content = match std::fs::read_to_string(&entry_file) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };

    let mut doc_parts = Vec::new();
    for line in content.lines().take(5) {
        let trimmed = line.trim();
        if let Some(stripped) = trimmed.strip_prefix("//!") {
            doc_parts.push(stripped.trim().to_string());
        }
    }
    doc_parts.join(" ").trim().to_string()
}

/// Extract only public API signatures from a `.rs` file, dropping function
/// bodies.
pub fn read_signatures_only(file_path: &Path) -> Result<String, FileOpsError> {
    let content = std::fs::read_to_string(file_path)
        .map_err(|e| FileOpsError::Io(format!("failed to read {}: {e}", file_path.display())))?;
    Ok(extract_signatures_from_content(&content))
}

/// Extract public API signatures from Rust source content.
pub fn extract_signatures_from_content(content: &str) -> String {
    extract_signatures(content)
}

/// Pre-computed workspace metadata. Built once per loop run and reused across
/// all task iterations so that Cargo.toml files are parsed only once.
#[derive(Clone)]
pub struct WorkspaceCache {
    pub members: Vec<String>,
    pub crate_names: HashMap<String, String>,
    pub crate_deps: HashMap<String, Vec<String>>,
    pub name_to_path: HashMap<String, String>,
    pub workspace_map_text: String,
    pub member_count: usize,
}

impl WorkspaceCache {
    pub fn build(project_root: &str) -> Result<Self, FileOpsError> {
        let root = Path::new(project_root);
        let meta = match parse_workspace_metadata(root) {
            Some(m) => m,
            None => return Ok(Self::empty()),
        };
        let name_to_path: HashMap<String, String> = meta
            .crate_names
            .iter()
            .map(|(p, n)| (n.clone(), p.clone()))
            .collect();
        let workspace_map_text = format_workspace_map(&meta, &name_to_path);
        let member_count = meta.members.len();
        Ok(Self {
            members: meta.members,
            crate_names: meta.crate_names,
            crate_deps: meta.crate_deps,
            name_to_path,
            workspace_map_text,
            member_count,
        })
    }

    pub async fn build_async(project_root: &str) -> Result<Self, FileOpsError> {
        let root = project_root.to_string();
        tokio::task::spawn_blocking(move || Self::build(&root))
            .await
            .map_err(|e| FileOpsError::Io(format!("spawn_blocking: {e}")))?
    }

    pub fn empty() -> Self {
        Self {
            members: Vec::new(),
            crate_names: HashMap::new(),
            crate_deps: HashMap::new(),
            name_to_path: HashMap::new(),
            workspace_map_text: String::new(),
            member_count: 1,
        }
    }
}

/// Count the number of workspace member crates by parsing the root Cargo.toml.
/// Returns 1 (single crate) if no workspace is detected.
pub fn count_workspace_members(project_root: &str) -> Result<usize, FileOpsError> {
    let root_cargo = Path::new(project_root).join("Cargo.toml");
    let content =
        std::fs::read_to_string(&root_cargo).map_err(|e| FileOpsError::Io(e.to_string()))?;
    let members = parse_workspace_members(&content);
    if members.is_empty() {
        Ok(1)
    } else {
        Ok(members.len())
    }
}

// ---------------------------------------------------------------------------
// Inlined Rust signature extraction (originally from aura_core::rust_signatures)
// ---------------------------------------------------------------------------

fn extract_signatures(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut output = String::new();
    let mut i = 0;

    while i < lines.len() {
        if let Some(next) = try_process_preamble(&lines, i, &mut output) {
            i = next;
        } else if let Some(next) = try_process_definition(&lines, i, &mut output) {
            i = next;
        } else {
            i += 1;
        }
    }

    output
}

fn try_process_preamble(lines: &[&str], i: usize, output: &mut String) -> Option<usize> {
    let trimmed = lines[i].trim();

    if trimmed.starts_with("use ")
        || trimmed.starts_with("pub use ")
        || trimmed.starts_with("pub mod ")
        || trimmed.starts_with("mod ")
        || trimmed.starts_with("//!")
    {
        output.push_str(trimmed);
        output.push('\n');
        return Some(i + 1);
    }

    if trimmed.starts_with("//") || trimmed.is_empty() {
        return Some(i + 1);
    }

    if trimmed.starts_with("#[") {
        push_with_line(output, i, trimmed);
        return Some(i + 1);
    }

    None
}

fn try_process_definition(lines: &[&str], i: usize, output: &mut String) -> Option<usize> {
    let trimmed = lines[i].trim();

    if (trimmed.starts_with("pub struct ") || trimmed.starts_with("pub enum "))
        && !trimmed.ends_with(';')
    {
        let (block, end) = sig_extract_braced_block(lines, i);
        push_with_line(output, i, &block);
        return Some(end + 1);
    }

    if trimmed.starts_with("pub trait ") {
        let (block, end) = extract_trait_signatures(lines, i);
        push_with_line(output, i, &block);
        return Some(end + 1);
    }

    if trimmed.starts_with("impl ") || trimmed.starts_with("impl<") {
        let (block, end) = extract_impl_signatures(lines, i);
        if !block.is_empty() {
            push_with_line(output, i, &block);
        }
        return Some(end + 1);
    }

    if trimmed.starts_with("pub fn ")
        || trimmed.starts_with("pub async fn ")
        || trimmed.starts_with("pub const fn ")
        || trimmed.starts_with("pub unsafe fn ")
    {
        let sig = extract_fn_signature(lines, i);
        let formatted = format!("{sig} {{ ... }}");
        push_with_line(output, i, &formatted);
        return Some(skip_braced_block(lines, i) + 1);
    }

    if trimmed.starts_with("pub type ") || trimmed.starts_with("pub const ") {
        push_with_line(output, i, trimmed);
        return Some(i + 1);
    }

    None
}

fn push_with_line(output: &mut String, line_idx: usize, content: &str) {
    let line_num = line_idx + 1;
    if content.contains('\n') {
        let first_line = content.lines().next().unwrap_or("");
        output.push_str(&format!("L{line_num}: {first_line}\n"));
        for rest in content.lines().skip(1) {
            output.push_str(rest);
            output.push('\n');
        }
    } else {
        output.push_str(&format!("L{line_num}: {content}\n"));
    }
}

fn sig_extract_braced_block(lines: &[&str], start: usize) -> (String, usize) {
    let mut depth: i32 = 0;
    let mut result = String::new();
    let mut started = false;

    for (j, line) in lines.iter().enumerate().skip(start) {
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    started = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        result.push_str(line.trim());
        result.push('\n');
        if started && depth <= 0 {
            return (result, j);
        }
    }
    (result, lines.len().saturating_sub(1))
}

fn extract_trait_signatures(lines: &[&str], start: usize) -> (String, usize) {
    extract_method_signatures(lines, start, false)
}

fn extract_impl_signatures(lines: &[&str], start: usize) -> (String, usize) {
    let header = lines[start].trim();
    let is_trait_impl = header.contains(" for ");

    if !impl_has_pub_methods(lines, start, is_trait_impl) && !is_trait_impl {
        let end = skip_braced_block(lines, start);
        return (String::new(), end);
    }

    extract_method_signatures(lines, start, true)
}

fn impl_has_pub_methods(lines: &[&str], start: usize, is_trait_impl: bool) -> bool {
    let mut depth: i32 = 0;
    let mut started = false;
    let mut in_fn_body = false;
    let mut fn_body_depth: i32 = 0;

    for line in lines.iter().skip(start) {
        let trimmed = line.trim();

        for ch in trimmed.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    started = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }

        if in_fn_body {
            fn_body_depth += trimmed.chars().filter(|&c| c == '{').count() as i32;
            fn_body_depth -= trimmed.chars().filter(|&c| c == '}').count() as i32;
            if fn_body_depth <= 0 {
                in_fn_body = false;
            }
            if started && depth <= 0 {
                break;
            }
            continue;
        }

        if started && depth > 1 {
            let is_fn = is_fn_start(trimmed);
            if is_fn {
                if trimmed.starts_with("pub ") || is_trait_impl {
                    return true;
                }
                if trimmed.contains('{') {
                    fn_body_depth = trimmed.chars().filter(|&c| c == '{').count() as i32
                        - trimmed.chars().filter(|&c| c == '}').count() as i32;
                    if fn_body_depth > 0 {
                        in_fn_body = true;
                    }
                }
            }
        }

        if started && depth <= 0 {
            break;
        }
    }
    false
}

fn format_method_entry(trimmed: &str) -> (String, i32) {
    let mut formatted = String::from("    ");
    if trimmed.contains('{') {
        let sig_part = match trimmed.find('{') {
            Some(pos) => trimmed[..pos].trim(),
            None => trimmed,
        };
        formatted.push_str(sig_part);
        formatted.push_str(" { ... }\n");
    } else {
        formatted.push_str(trimmed);
        formatted.push('\n');
    }
    let body_depth = trimmed.chars().filter(|&c| c == '{').count() as i32
        - trimmed.chars().filter(|&c| c == '}').count() as i32;
    (formatted, body_depth)
}

fn is_impl_noise(trimmed: &str, header: &str) -> bool {
    !trimmed.is_empty()
        && !trimmed.starts_with("pub ")
        && !trimmed.starts_with("fn ")
        && !trimmed.starts_with("async fn ")
        && !trimmed.starts_with("type ")
        && !trimmed.starts_with("const ")
        && !trimmed.starts_with("//")
        && !trimmed.starts_with('}')
        && !trimmed.starts_with('{')
        && !header.contains(trimmed)
}

fn extract_method_signatures(
    lines: &[&str],
    start: usize,
    filter_impl_noise: bool,
) -> (String, usize) {
    let mut depth: i32 = 0;
    let mut result = String::new();
    let mut started = false;
    let mut in_fn_body = false;
    let mut fn_body_depth: i32 = 0;
    let header = lines.get(start).map_or("", |l| l.trim());

    for (j, line) in lines.iter().enumerate().skip(start) {
        let trimmed = line.trim();

        for ch in trimmed.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    started = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }

        if in_fn_body {
            fn_body_depth += trimmed.chars().filter(|&c| c == '{').count() as i32;
            fn_body_depth -= trimmed.chars().filter(|&c| c == '}').count() as i32;
            if fn_body_depth <= 0 {
                in_fn_body = false;
            }
            if started && depth <= 0 {
                result.push_str("}\n");
                return (result, j);
            }
            continue;
        }

        if started && depth > 1 && is_fn_start(trimmed) {
            let (entry, body_depth) = format_method_entry(trimmed);
            result.push_str(&entry);
            if trimmed.contains('{') && body_depth > 0 {
                in_fn_body = true;
                fn_body_depth = body_depth;
            }
            if started && depth <= 0 {
                return (result, j);
            }
            continue;
        }

        if filter_impl_noise
            && started
            && depth >= 1
            && j != start
            && is_impl_noise(trimmed, header)
        {
            if started && depth <= 0 {
                result.push_str("}\n");
                return (result, j);
            }
            continue;
        }

        result.push_str(trimmed);
        result.push('\n');

        if started && depth <= 0 {
            return (result, j);
        }
    }
    (result, lines.len().saturating_sub(1))
}

fn is_fn_start(trimmed: &str) -> bool {
    trimmed.starts_with("pub fn ")
        || trimmed.starts_with("pub async fn ")
        || trimmed.starts_with("pub const fn ")
        || trimmed.starts_with("pub(crate) fn ")
        || trimmed.starts_with("pub(crate) async fn ")
        || trimmed.starts_with("fn ")
        || trimmed.starts_with("async fn ")
}

fn extract_fn_signature(lines: &[&str], start: usize) -> String {
    let mut sig = String::new();
    for line in lines.iter().skip(start) {
        let trimmed = line.trim();
        if let Some(pos) = trimmed.find('{') {
            let before = trimmed[..pos].trim();
            if !before.is_empty() {
                if !sig.is_empty() {
                    sig.push(' ');
                }
                sig.push_str(before);
            }
            break;
        }
        if !sig.is_empty() {
            sig.push(' ');
        }
        sig.push_str(trimmed);
    }
    sig
}

fn skip_braced_block(lines: &[&str], start: usize) -> usize {
    let mut depth: i32 = 0;
    let mut started = false;
    for (j, line) in lines.iter().enumerate().skip(start) {
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    started = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if started && depth <= 0 {
            return j;
        }
    }
    lines.len().saturating_sub(1)
}
