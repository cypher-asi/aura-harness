mod file_search;

use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::ToolDefinition;
use aura_core_types::ToolResult;
use file_search::{candidate_files, FileSearch};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use tracing::{debug, instrument};

/// Maximum compiled regex size (bytes) accepted by `search_code`.
const SEARCH_REGEX_SIZE_LIMIT: usize = 1_000_000;

/// Directories automatically skipped during code search.
const SEARCH_SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "__pycache__",
    "dist",
    "build",
    ".next",
    "vendor",
    ".venv",
    "coverage",
    ".tox",
    ".mypy_cache",
];

/// Most results one call returns, whatever `max_results` asks for.
const MAX_RESULTS_CEILING: usize = 500;

/// Longest line `search_code` prints. Longer lines (minified code, data
/// files) are cut so a single match cannot flood the context.
const MAX_LINE_CHARS: usize = 300;

/// What `search_code` returns for the files it matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Matching lines as `path:line:text`, or blocks with context lines.
    Content,
    /// Each matching file once.
    FilesWithMatches,
    /// Each matching file with its number of matching lines.
    Count,
}

impl OutputMode {
    fn parse(raw: Option<&str>) -> Result<Self, ToolError> {
        match raw {
            None | Some("content") => Ok(Self::Content),
            Some("files_with_matches") => Ok(Self::FilesWithMatches),
            Some("count") => Ok(Self::Count),
            Some(other) => Err(ToolError::InvalidArguments(format!(
                "Invalid output_mode '{other}': use content, files_with_matches or count"
            ))),
        }
    }
}

fn should_skip_dir(name: &str) -> bool {
    SEARCH_SKIP_DIRS.contains(&name)
}

/// `line` cut to [`MAX_LINE_CHARS`] with a note of how much was left off.
fn clip_line(line: &str) -> Cow<'_, str> {
    if line.len() <= MAX_LINE_CHARS {
        return Cow::Borrowed(line);
    }
    let mut end = MAX_LINE_CHARS;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!("{}… [+{} chars]", &line[..end], line.len() - end))
}

/// Format a single match with context lines.
fn format_match_with_context(
    relative_path: &str,
    lines: &[&str],
    line_idx: usize,
    context: usize,
) -> String {
    use std::fmt::Write;

    let start = line_idx.saturating_sub(context);
    let end = (line_idx + context + 1).min(lines.len());
    let mut block = format!("{relative_path}:{}", line_idx + 1);
    for (ctx_idx, ctx_line) in lines[start..end].iter().enumerate() {
        let abs_idx = start + ctx_idx;
        let marker = if abs_idx == line_idx { ">" } else { " " };
        let _ = write!(
            block,
            "\n{marker} {:>4}|{}",
            abs_idx + 1,
            clip_line(ctx_line)
        );
    }
    block
}

/// Build a diagnostic message when `search_code` finds zero matches.
fn zero_match_diagnostic(sandbox: &Sandbox, path: Option<&str>, pattern: &str) -> String {
    use std::fmt::Write;

    let mut msg = String::from("No matches found");
    if let Some(p) = path {
        let resolved = sandbox.resolve(p);
        if resolved.is_err() || !resolved.as_ref().is_ok_and(|r| r.exists()) {
            let _ = write!(msg, ". Note: path '{p}' does not exist");
        }
    }
    if pattern.contains('\\') || pattern.contains('[') || pattern.contains('(') {
        msg.push_str(". Tip: check that special regex characters are escaped correctly");
    }
    msg
}

/// Marker appended when `search_code` hits the `max_results` cap, so the model
/// knows the result set is capped (not exhaustive) and how to recover the rest.
/// Mirrors the truncation signal `fs_read` already emits in `read.rs`.
fn search_truncation_marker(max_results: usize, mode: OutputMode) -> String {
    let unit = match mode {
        OutputMode::Content => "matches",
        OutputMode::FilesWithMatches | OutputMode::Count => "files",
    };
    format!(
        "\n... [truncated at {max_results} {unit} (max_results cap); more may exist — refine the pattern, narrow with path/file_pattern, or use output_mode files_with_matches first.]"
    )
}

/// Translate a file-name glob (`*`, `?`, `{a,b}`) into an anchored regex,
/// escaping every other character literally.
fn glob_to_regex(glob: &str) -> String {
    let mut out = String::from("^");
    let mut brace_depth = 0usize;
    for c in glob.chars() {
        match c {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '{' => {
                brace_depth += 1;
                out.push_str("(?:");
            }
            '}' if brace_depth > 0 => {
                brace_depth -= 1;
                out.push(')');
            }
            ',' if brace_depth > 0 => out.push('|'),
            _ => out.push_str(&regex::escape(c.encode_utf8(&mut [0; 4]))),
        }
    }
    out.push('$');
    out
}

/// Search for patterns in code.
///
/// Walks the tree honouring `.gitignore` and [`SEARCH_SKIP_DIRS`]. In
/// [`OutputMode::Content`], `context_lines` (0-10) adds surrounding lines
/// with `>` marking each match line. `max_results` counts matches in
/// content mode and files otherwise, and never exceeds
/// [`MAX_RESULTS_CEILING`].
#[instrument(skip(sandbox), fields(pattern = %pattern))]
pub fn search_code(
    sandbox: &Sandbox,
    pattern: &str,
    path: Option<&str>,
    file_pattern: Option<&str>,
    max_results: usize,
    context_lines: usize,
    mode: OutputMode,
) -> Result<ToolResult, ToolError> {
    let context_lines = context_lines.min(10);
    let max_results = max_results.clamp(1, MAX_RESULTS_CEILING);

    let search_root = match path {
        Some(p) => sandbox.resolve_existing(p)?,
        None => sandbox.root().to_path_buf(),
    };

    debug!(?search_root, "Searching code");

    let regex = compile_pattern(pattern)?;
    let file_pattern_regex = file_pattern
        .map(|p| regex::Regex::new(&glob_to_regex(p)))
        .transpose()
        .map_err(|e| ToolError::InvalidArguments(format!("Invalid file pattern: {e}")))?;

    let search = FileSearch {
        root: &search_root,
        regex: &regex,
        mode,
        context_lines,
        limit: max_results,
    };
    let files = candidate_files(&search_root, file_pattern_regex.as_ref());
    let (results, match_count) = search.collect(files);

    if results.is_empty() {
        let msg = zero_match_diagnostic(sandbox, path, pattern);
        return Ok(
            ToolResult::success("search_code", msg).with_metadata("match_count", "0".to_string())
        );
    }
    Ok(search_result(&results, match_count, max_results, mode))
}

/// `pattern` compiled (from the cache when possible), within
/// [`SEARCH_REGEX_SIZE_LIMIT`].
fn compile_pattern(pattern: &str) -> Result<regex::Regex, ToolError> {
    let regex = cached_regex(pattern)
        .map_err(|e| ToolError::InvalidArguments(format!("Invalid regex: {e}")))?;

    if regex.as_str().len() > SEARCH_REGEX_SIZE_LIMIT {
        return Err(ToolError::InvalidArguments(format!(
            "Regex pattern exceeds size limit of {SEARCH_REGEX_SIZE_LIMIT} bytes"
        )));
    }
    Ok(regex)
}

/// The success result for a search that found `results`.
fn search_result(
    results: &[String],
    match_count: usize,
    max_results: usize,
    mode: OutputMode,
) -> ToolResult {
    let truncated = results.len() >= max_results;
    let mut output = results.join("\n");
    if truncated {
        output.push_str(&search_truncation_marker(max_results, mode));
    }
    let mut result = ToolResult::success("search_code", output)
        .with_metadata("match_count", match_count.to_string());
    if mode != OutputMode::Content {
        result = result.with_metadata("file_count", results.len().to_string());
    }
    if truncated {
        result = result.with_metadata("truncated", "true");
    }
    result
}

/// Compiled patterns kept between calls; agents repeat their searches.
const REGEX_CACHE_SIZE: usize = 64;

/// `pattern` compiled, from the cache when it was compiled before.
fn cached_regex(pattern: &str) -> Result<regex::Regex, regex::Error> {
    static CACHE: OnceLock<Mutex<HashMap<String, regex::Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some(regex) = cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(pattern)
    {
        return Ok(regex.clone());
    }
    let regex = regex::Regex::new(pattern)?;
    let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if cache.len() >= REGEX_CACHE_SIZE {
        cache.clear();
    }
    cache.insert(pattern.to_string(), regex.clone());
    Ok(regex)
}

/// `search_code` tool: search for patterns in code.
pub struct SearchCodeTool;

#[async_trait]
impl Tool for SearchCodeTool {
    fn name(&self) -> &str {
        "search_code"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "search_code".into(),
            description: "Search file contents with a regex, skipping files the project's .gitignore excludes. For a broad question start with output_mode \"files_with_matches\" (or \"count\") to see where matches are, then search a narrower path or read just the lines you need. Content mode returns path:line:text per match, with long lines cut.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Search pattern (regex supported)"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (default: workspace root)"
                    },
                    "file_pattern": {
                        "type": "string",
                        "description": "Glob pattern for files to search (e.g., '*.rs', '*.ts')"
                    },
                    "include": {
                        "type": "string",
                        "description": "Glob pattern for files to search (alias for file_pattern)"
                    },
                    "output_mode": {
                        "type": "string",
                        "enum": ["content", "files_with_matches", "count"],
                        "description": "content (default): matching lines; files_with_matches: each matching file once; count: matching lines per file"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum matches (content) or files (other modes) to return (default: 100, at most 500)"
                    },
                    "context_lines": {
                        "type": "integer",
                        "description": "Surrounding lines to show per match in content mode (0-10, default: 0)"
                    }
                },
                "required": ["pattern"]
            }),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'pattern' argument".into()))?
            .to_string();
        let path = args["path"].as_str().map(String::from);
        let file_pattern = args["include"]
            .as_str()
            .or_else(|| args["file_pattern"].as_str())
            .map(String::from);
        let max_results = args["max_results"]
            .as_u64()
            .map_or(100, |n| usize::try_from(n).unwrap_or(100));
        let context_lines = args["context_lines"]
            .as_u64()
            .map_or(0, |n| usize::try_from(n).unwrap_or(0));
        let mode = OutputMode::parse(args["output_mode"].as_str())?;
        let sandbox = ctx.sandbox.clone();
        super::spawn_blocking_tool(move || {
            search_code(
                &sandbox,
                &pattern,
                path.as_deref(),
                file_pattern.as_deref(),
                max_results,
                context_lines,
                mode,
            )
        })
        .await
    }
}

#[cfg(test)]
mod tests;
