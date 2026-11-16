//! Project instructions: the `AGENTS.md`, `CLAUDE.md` and Cursor rules
//! files that tell agents how to work in a project.
//!
//! An agent's instructions come from its working folder and every folder
//! above it, up to (not including) the home directory, plus the user's own
//! `AURA_HOME/AGENTS.md`. They go into the system prompt from the most
//! general to the most specific, so the file nearest the working folder
//! has the last word.
//!
//! Every turn checks the files' sizes and modification times and re-reads
//! them only when something changed. The section depends on nothing but
//! the files, so while they stay the same it is byte-identical from turn
//! to turn and the system prompt keeps hitting the prompt cache.

use aura_context_prompts::system::sections::project_instructions::{
    render, InstructionFile, OnRequestRule, ProjectInstructionsView,
};
use aura_ui_terminal::events::{InstructionFileView, InstructionStatus};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;
use tracing::{debug, info, warn};

/// Most bytes of instructions one system prompt carries. Instruction files
/// ride along on every model call, so a runaway file must not eat the
/// context window.
pub const DEFAULT_MAX_BYTES: usize = 32 * 1024;

/// A file that no longer fits whole is cut only when at least this much of
/// it fits; a shorter start rarely holds a usable instruction.
const MIN_PARTIAL_BYTES: usize = 1024;

/// Turns loading off when set to `0`, `off`, `false` or `no`.
pub const ENV_SWITCH: &str = "AURA_PROJECT_INSTRUCTIONS";

/// Names an `AGENTS.md` may have; the first one found in a folder wins.
const AGENTS_NAMES: &[&str] = &["AGENTS.md", "agents.md"];

/// Where instructions come from and how much of them to load.
#[derive(Debug, Clone)]
pub struct InstructionSettings {
    /// Whether to load any
    pub enabled: bool,
    /// The user's instructions for every project (`AURA_HOME/AGENTS.md`)
    pub user_file: Option<PathBuf>,
    /// Home directory: the walk up from a working folder stops below it,
    /// and paths under it are shown with `~`
    pub home: Option<PathBuf>,
    /// Most bytes of instructions in one system prompt
    pub max_bytes: usize,
}

impl InstructionSettings {
    /// Settings for this process: [`ENV_SWITCH`], the home directory, and
    /// `AGENTS.md` under `aura_home`.
    #[must_use]
    pub fn from_env(aura_home: Option<&Path>) -> Self {
        let enabled = std::env::var(ENV_SWITCH).map_or(true, |value| !is_off(&value));
        Self {
            enabled,
            user_file: aura_home.map(|home| home.join("AGENTS.md")),
            home: dirs::home_dir(),
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// Settings that load nothing.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            user_file: None,
            home: None,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

fn is_off(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "off" | "false" | "no"
    )
}

/// The instructions of one working folder, as of the last check.
#[derive(Debug, Default)]
pub struct LoadedInstructions {
    stamps: Vec<Stamp>,
    /// The `<project_instructions>` system prompt section; empty when no
    /// file was found
    pub text: String,
    /// Every file found and how the section uses it
    pub files: Vec<InstructionFileView>,
}

/// Loads and caches project instructions per working folder.
#[derive(Debug)]
pub struct ProjectInstructions {
    settings: InstructionSettings,
    /// `settings.home` with symlinks resolved, to compare with resolved
    /// working folders
    real_home: Option<PathBuf>,
    cache: Mutex<HashMap<PathBuf, Arc<LoadedInstructions>>>,
}

impl ProjectInstructions {
    #[must_use]
    pub fn new(settings: InstructionSettings) -> Self {
        let real_home = settings
            .home
            .as_ref()
            .map(|home| std::fs::canonicalize(home).unwrap_or_else(|_| home.clone()));
        Self {
            settings,
            real_home,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Instructions from `folder` alone. Treating it as the home directory
    /// keeps the folders above it and the user's own files out of tests.
    #[cfg(test)]
    pub(crate) fn only_in(folder: &Path) -> Self {
        Self::new(InstructionSettings {
            enabled: true,
            user_file: None,
            home: Some(folder.to_path_buf()),
            max_bytes: DEFAULT_MAX_BYTES,
        })
    }

    /// The instructions for an agent working in `workspace`, re-read only
    /// when a file was added, removed or changed since the last call.
    pub fn for_workspace(&self, workspace: &Path) -> Arc<LoadedInstructions> {
        if !self.settings.enabled {
            return Arc::default();
        }
        let candidates = self.candidates(workspace);
        let stamps: Vec<Stamp> = candidates.iter().map(|c| Stamp::of(&c.path)).collect();
        if let Some(cached) = lock(&self.cache).get(workspace) {
            if cached.stamps == stamps {
                return Arc::clone(cached);
            }
        }
        let loaded = Arc::new(self.load(&candidates, stamps));
        info!(
            workspace = %workspace.display(),
            files = loaded.files.len(),
            bytes = loaded.text.len(),
            "Loaded project instructions"
        );
        lock(&self.cache).insert(workspace.to_path_buf(), Arc::clone(&loaded));
        loaded
    }

    /// Instruction files for `workspace`, most general first.
    fn candidates(&self, workspace: &Path) -> Vec<Candidate> {
        let workspace =
            std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
        let mut folders = Vec::new();
        for folder in workspace.ancestors() {
            let at_home = self.real_home.as_deref() == Some(folder);
            if at_home && !folders.is_empty() {
                break;
            }
            folders.push(folder);
            // An agent working in the home folder itself reads its files,
            // but nothing above it.
            if at_home {
                break;
            }
        }
        folders.reverse();

        let mut found = Vec::new();
        if let Some(user) = self.settings.user_file.as_ref().filter(|p| p.is_file()) {
            found.push(Candidate::plain(user.clone()));
        }
        for folder in folders {
            folder_candidates(folder, &mut found);
        }
        // `CLAUDE.md` is often a symlink to `AGENTS.md`; keep one copy.
        let mut seen = HashSet::new();
        found.retain(|c| {
            seen.insert(std::fs::canonicalize(&c.path).unwrap_or_else(|_| c.path.clone()))
        });
        found
    }

    fn load(&self, candidates: &[Candidate], stamps: Vec<Stamp>) -> LoadedInstructions {
        let sources: Vec<Source> = candidates.iter().filter_map(read_source).collect();
        let uses = budget(&sources, self.settings.max_bytes);
        let paths: Vec<String> = sources
            .iter()
            .map(|s| s.path.display().to_string())
            .collect();

        let mut files = Vec::new();
        let mut on_request = Vec::new();
        let mut omitted = Vec::new();
        let mut views = Vec::new();
        for ((source, use_), path) in sources.iter().zip(&uses).zip(&paths) {
            let (bytes, status) = match *use_ {
                Use::Whole => {
                    files.push(InstructionFile {
                        path,
                        globs: source.globs(),
                        body: &source.body,
                        truncated_from: None,
                    });
                    (source.body.len(), InstructionStatus::Included)
                }
                Use::Start(cut) => {
                    files.push(InstructionFile {
                        path,
                        globs: source.globs(),
                        body: &source.body[..cut],
                        truncated_from: Some(source.body.len()),
                    });
                    (cut, InstructionStatus::Truncated(source.body.len()))
                }
                Use::OnRequest => {
                    if let Kind::OnRequest(description) = &source.kind {
                        on_request.push(OnRequestRule { path, description });
                    }
                    (0, InstructionStatus::OnRequest)
                }
                Use::Omitted => {
                    omitted.push(path.as_str());
                    (0, InstructionStatus::Omitted)
                }
            };
            views.push(InstructionFileView {
                path: self.label(&source.path),
                bytes,
                globs: source.globs().map(str::to_string),
                status,
            });
        }
        let text = render(&ProjectInstructionsView {
            files: &files,
            on_request: &on_request,
            omitted: &omitted,
        })
        .unwrap_or_default();
        LoadedInstructions {
            stamps,
            text,
            files: views,
        }
    }

    /// `path` with the home directory shortened to `~`.
    fn label(&self, path: &Path) -> String {
        [self.real_home.as_deref(), self.settings.home.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|home| path.strip_prefix(home).ok())
            .map_or_else(
                || path.display().to_string(),
                |rest| format!("~/{}", rest.display()),
            )
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a file looked like when it was read: a different size or time
/// means it changed. `None` fields mean it could not be inspected.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: Option<u64>,
}

impl Stamp {
    fn of(path: &Path) -> Self {
        let meta = std::fs::metadata(path).ok();
        Self {
            path: path.to_path_buf(),
            modified: meta.as_ref().and_then(|m| m.modified().ok()),
            len: meta.map(|m| m.len()),
        }
    }
}

/// A file that may hold instructions.
#[derive(Debug)]
struct Candidate {
    path: PathBuf,
    /// A Cursor rule, whose frontmatter says when it applies
    rule: bool,
}

impl Candidate {
    const fn plain(path: PathBuf) -> Self {
        Self { path, rule: false }
    }

    const fn rule(path: PathBuf) -> Self {
        Self { path, rule: true }
    }
}

/// The `AGENTS.md` directly in `folder`, if it has one.
#[must_use]
pub fn agents_md_in(folder: &Path) -> Option<PathBuf> {
    AGENTS_NAMES
        .iter()
        .map(|name| folder.join(name))
        .find(|path| path.is_file())
}

/// The instruction files directly in `folder`, in the order they apply.
fn folder_candidates(folder: &Path, found: &mut Vec<Candidate>) {
    if let Some(agents) = agents_md_in(folder) {
        found.push(Candidate::plain(agents));
    }
    let claude = folder.join("CLAUDE.md");
    if claude.is_file() {
        found.push(Candidate::plain(claude));
    }
    let cursor = folder.join(".cursor");
    let rules_md = cursor.join("rules.md");
    if rules_md.is_file() {
        found.push(Candidate::rule(rules_md));
    }
    let Ok(entries) = std::fs::read_dir(cursor.join("rules")) else {
        return;
    };
    let mut rules: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext == "md" || ext == "mdc")
        })
        .collect();
    // Directory order varies by filesystem; the prompt must not.
    rules.sort();
    found.extend(rules.into_iter().map(Candidate::rule));
}

/// When a file's instructions apply.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Always,
    /// Only for files matching these globs
    Scoped(String),
    /// When the work matches this description
    OnRequest(String),
}

/// A file read for the prompt.
#[derive(Debug)]
struct Source {
    path: PathBuf,
    kind: Kind,
    /// Content without frontmatter
    body: String,
}

impl Source {
    fn globs(&self) -> Option<&str> {
        match &self.kind {
            Kind::Scoped(globs) => Some(globs),
            _ => None,
        }
    }
}

fn read_source(candidate: &Candidate) -> Option<Source> {
    let content = match std::fs::read_to_string(&candidate.path) {
        Ok(content) => content,
        Err(e) => {
            warn!(path = %candidate.path.display(), error = %e, "Skipping unreadable instruction file");
            return None;
        }
    };
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    let (kind, body) = if candidate.rule {
        let (meta, body) = split_frontmatter(content);
        let Some(kind) = rule_kind(meta) else {
            debug!(path = %candidate.path.display(), "Skipping rule that only applies when named");
            return None;
        };
        (kind, body)
    } else {
        (Kind::Always, content)
    };
    let body = body.trim();
    if body.is_empty() && !matches!(kind, Kind::OnRequest(_)) {
        return None;
    }
    Some(Source {
        path: candidate.path.clone(),
        kind,
        body: body.to_string(),
    })
}

/// `content` split into its `---` frontmatter (if any) and the rest.
fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let Some(rest) = content
        .strip_prefix("---\r\n")
        .or_else(|| content.strip_prefix("---\n"))
    else {
        return (None, content);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return (Some(&rest[..offset]), &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    (None, content)
}

/// The frontmatter fields Cursor rules use. Parsed by hand: Cursor writes
/// `globs: **/*.rs` unquoted, which is not valid YAML.
#[derive(Debug, Default)]
struct RuleMeta {
    description: String,
    globs: Vec<String>,
    always_apply: bool,
}

fn parse_rule_meta(frontmatter: &str) -> RuleMeta {
    let mut meta = RuleMeta::default();
    let mut in_globs_list = false;
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if in_globs_list {
            if let Some(item) = trimmed.strip_prefix('-') {
                meta.globs.extend(split_globs(item));
                continue;
            }
            in_globs_list = false;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "description" => meta.description = unquote(value).to_string(),
            "globs" if value.is_empty() => in_globs_list = true,
            "globs" => meta.globs.extend(split_globs(value)),
            "alwaysApply" => meta.always_apply = value.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    meta
}

fn split_globs(value: &str) -> impl Iterator<Item = String> + '_ {
    let value = value.trim();
    let value = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(value);
    value
        .split(',')
        .map(unquote)
        .filter(|glob| !glob.is_empty())
        .map(str::to_string)
}

fn unquote(value: &str) -> &str {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|v| v.strip_suffix(quote))
        {
            return inner.trim();
        }
    }
    value
}

/// When a rule applies, per its frontmatter. `None` for rules that apply
/// only when the user names them.
fn rule_kind(frontmatter: Option<&str>) -> Option<Kind> {
    let Some(frontmatter) = frontmatter else {
        return Some(Kind::Always);
    };
    let meta = parse_rule_meta(frontmatter);
    if meta.always_apply {
        Some(Kind::Always)
    } else if !meta.globs.is_empty() {
        Some(Kind::Scoped(meta.globs.join(", ")))
    } else if !meta.description.is_empty() {
        Some(Kind::OnRequest(meta.description))
    } else {
        None
    }
}

/// How much of a file the prompt carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Use {
    Whole,
    /// Its first this many bytes
    Start(usize),
    OnRequest,
    Omitted,
}

/// Share `max_bytes` among `sources` (most general first), nearest first,
/// so the instructions that win on conflict are the last to be cut.
fn budget(sources: &[Source], max_bytes: usize) -> Vec<Use> {
    let mut uses = vec![Use::Omitted; sources.len()];
    let mut remaining = max_bytes;
    for (use_, source) in uses.iter_mut().zip(sources).rev() {
        *use_ = if matches!(source.kind, Kind::OnRequest(_)) {
            Use::OnRequest
        } else if source.body.len() <= remaining {
            remaining -= source.body.len();
            Use::Whole
        } else if remaining >= MIN_PARTIAL_BYTES {
            let cut = cut_point(&source.body, remaining);
            remaining = 0;
            Use::Start(cut)
        } else {
            Use::Omitted
        };
    }
    uses
}

/// Where to cut `body` to at most `limit` bytes: at a line end when one
/// is in the second half, else at a character boundary.
fn cut_point(body: &str, limit: usize) -> usize {
    let mut end = limit.min(body.len());
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    match body[..end].rfind('\n') {
        Some(newline) if newline >= end / 2 => newline,
        _ => end,
    }
}

#[cfg(test)]
mod tests;
