//! Reading and matching the files of one `search_code` call.

use super::{clip_line, format_match_with_context, should_skip_dir, OutputMode};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

/// Maximum file size (bytes) that `search_code` will read into memory.
const MAX_SEARCH_FILE_SIZE: u64 = 5 * 1024 * 1024;

/// Files `search_code` reads in parallel before taking their hits.
const SEARCH_CHUNK_FILES: usize = 64;

/// Text files under `root` in walk order, whose names match
/// `file_pattern_regex` when one is given.
pub(super) fn candidate_files<'a>(
    root: &Path,
    file_pattern_regex: Option<&'a regex::Regex>,
) -> impl Iterator<Item = PathBuf> + 'a {
    crate::fs_tools::walk::walk(root, should_skip_dir)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .map(ignore::DirEntry::into_path)
        .filter(move |path| {
            file_pattern_regex.is_none_or(|fp_regex| {
                fp_regex.is_match(path.file_name().and_then(|n| n.to_str()).unwrap_or(""))
            })
        })
        .filter(|path| is_text_file(path))
}

/// What one file gave a search: the lines to print, and how many
/// matches they stand for.
pub(super) struct FileHits {
    lines: Vec<String>,
    matches: usize,
}

/// One `search_code` call's pattern and output settings.
pub(super) struct FileSearch<'a> {
    pub(super) root: &'a Path,
    pub(super) regex: &'a regex::Regex,
    pub(super) mode: OutputMode,
    pub(super) context_lines: usize,
    /// Most lines one file can add, the call's `max_results`
    pub(super) limit: usize,
}

impl FileSearch<'_> {
    /// The output lines for `files` in walk order, at most `limit` of
    /// them, and how many matches they stand for.
    pub(super) fn collect(&self, mut files: impl Iterator<Item = PathBuf>) -> (Vec<String>, usize) {
        let max_results = self.limit;
        let mut results = Vec::new();
        let mut match_count = 0usize;
        // Files are read and matched in parallel a chunk at a time, and their
        // hits taken in walk order, so the result is what reading them one by
        // one gives.
        'chunks: while results.len() < max_results {
            let chunk: Vec<PathBuf> = files.by_ref().take(SEARCH_CHUNK_FILES).collect();
            if chunk.is_empty() {
                break;
            }
            let hits: Vec<Option<FileHits>> = chunk.par_iter().map(|p| self.file(p)).collect();
            for hits in hits.into_iter().flatten() {
                if results.len() >= max_results {
                    break 'chunks;
                }
                match self.mode {
                    OutputMode::Content => {
                        let room = max_results - results.len();
                        match_count += hits.lines.len().min(room);
                        results.extend(hits.lines.into_iter().take(room));
                    }
                    OutputMode::FilesWithMatches | OutputMode::Count => {
                        match_count += hits.matches;
                        results.extend(hits.lines);
                    }
                }
            }
        }
        (results, match_count)
    }

    /// The hits in the file at `path`, or `None` when it has none or
    /// can't be read as text.
    fn file(&self, path: &Path) -> Option<FileHits> {
        if path
            .metadata()
            .map_or(true, |m| m.len() > MAX_SEARCH_FILE_SIZE)
        {
            return None;
        }
        let file_content = fs::read_to_string(path).ok()?;
        let lines: Vec<&str> = file_content.lines().collect();
        let relative_path = path
            .strip_prefix(self.root)
            .unwrap_or(path)
            .to_string_lossy();
        let regex = self.regex;
        let hits = match self.mode {
            OutputMode::Content => self.content_hits(&relative_path, &lines),
            OutputMode::FilesWithMatches => FileHits {
                lines: vec![relative_path.into_owned()],
                matches: usize::from(lines.iter().any(|line| regex.is_match(line))),
            },
            OutputMode::Count => {
                let count = lines.iter().filter(|line| regex.is_match(line)).count();
                FileHits {
                    lines: vec![format!("{relative_path}:{count}")],
                    matches: count,
                }
            }
        };
        (hits.matches > 0).then_some(hits)
    }

    /// Content-mode hits: each matching line (with context when asked),
    /// at most `limit` of them.
    fn content_hits(&self, relative_path: &str, lines: &[&str]) -> FileHits {
        let out: Vec<String> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| self.regex.is_match(line))
            .take(self.limit)
            .map(|(line_idx, line)| {
                if self.context_lines == 0 {
                    format!("{relative_path}:{}:{}", line_idx + 1, clip_line(line))
                } else {
                    format_match_with_context(relative_path, lines, line_idx, self.context_lines)
                }
            })
            .collect();
        FileHits {
            matches: out.len(),
            lines: out,
        }
    }
}

/// Whether `search_code` should attempt to read a file.
///
/// Uses a denylist of known-binary extensions rather than an allowlist of text
/// extensions, so no source file is silently skipped just because its extension
/// (e.g. `.tsx`, `.jsx`, `.vue`, `.scss`) isn't on a hardcoded list — the old
/// allowlist dropped most of a typical web codebase. The caller's
/// `read_to_string` is the backstop for anything non-UTF-8.
pub(super) fn is_text_file(path: &std::path::Path) -> bool {
    const BINARY_EXTENSIONS: &[&str] = &[
        // images
        "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "tiff", "tif", "heic", "avif",
        // audio / video
        "mp3", "wav", "flac", "ogg", "m4a", "aac", "mp4", "mov", "avi", "mkv", "webm", "wmv",
        // archives / compressed
        "zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "lz4",
        // documents (binary)
        "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
        // executables / objects / libraries
        "exe", "dll", "so", "dylib", "a", "o", "obj", "bin", "class", "wasm", "node", "pyc", "pdb",
        // fonts
        "ttf", "otf", "woff", "woff2", "eot", // databases / binary data
        "db", "sqlite", "sqlite3", "lockb",
    ];
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    !BINARY_EXTENSIONS.contains(&extension.as_str())
}
