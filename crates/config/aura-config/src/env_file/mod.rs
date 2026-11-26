//! Loading `.env` files into the process environment.
//!
//! One shared loader for every binary (`aura`, `aura-desktop`,
//! `aura-node`), so the boxd API key and every other setting can live in
//! a `.env` file.
//!
//! ## Order
//!
//! 1. Variables already in the process environment. Nothing here
//!    overrides them.
//! 2. The `.env` found from the current working directory upward (what
//!    `dotenvy::dotenv()` did before).
//! 3. `AURA_HOME/.env` (e.g. `~/.aura/.env`), so the desktop app
//!    launched from a dock with no project folder still finds the key.
//!    `AURA_HOME` is resolved *after* step 2, so a project `.env` may
//!    set it. Skipped when it is the same file as step 2.
//!
//! Within a file the first occurrence of a name wins, and an earlier
//! file wins over a later one, because a variable that is set is never
//! overwritten.
//!
//! ## Invariants
//!
//! - Only file paths are recorded or logged, never values.
//! - [`load`] runs once per process; [`reload`] parses the files again so
//!   a variable newly added to a file is picked up. A variable that was
//!   already loaded keeps its first value, since loading never overrides.
//! - Reading is blocking file IO. Call [`reload`] from a blocking
//!   context (e.g. `tokio::task::spawn_blocking`).
//!
//! ## Failure modes
//!
//! A missing file is skipped silently. A file that cannot be read or
//! parsed, or that is larger than [`MAX_ENV_FILE_BYTES`], is skipped
//! with a warning in [`EnvFileReport::warnings`]. On Unix, a file holding
//! `BOXD_API_KEY` that other users can read produces a warning too.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use zeroize::Zeroizing;

use crate::aura_home::{AuraHome, AuraHomeError};
use crate::env::BOXD_API_KEY;

/// File name looked for in the working directory, its parents and
/// `AURA_HOME`.
const ENV_FILE_NAME: &str = ".env";

/// Largest `.env` file that is read. Anything bigger is skipped with a
/// warning rather than read into memory.
pub const MAX_ENV_FILE_BYTES: u64 = 1024 * 1024;

/// What a load pass did: the files it read and any warnings. Holds
/// paths and messages only, never values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvFileReport {
    /// Files that were found and parsed, in load order.
    pub loaded: Vec<PathBuf>,
    /// Problems worth showing the user (unreadable files, files holding
    /// the API key that other users can read).
    pub warnings: Vec<String>,
}

impl EnvFileReport {
    /// Files that were found and parsed, in load order.
    #[must_use]
    pub fn loaded_files(&self) -> &[PathBuf] {
        &self.loaded
    }

    /// Warnings from the load pass.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

static REPORT: OnceLock<EnvFileReport> = OnceLock::new();

/// Load the `.env` files into the process environment, once per
/// process. Later calls return the first report.
///
/// Call it at the top of `main`, before any threads start: it sets
/// process environment variables.
pub fn load() -> &'static EnvFileReport {
    REPORT.get_or_init(load_process_env)
}

/// The report of [`load`], if it has run.
#[must_use]
pub fn report() -> Option<&'static EnvFileReport> {
    REPORT.get()
}

/// Parse the `.env` files again and set any variable that is still
/// unset. Use it to pick up a value added to a file after start-up.
///
/// Does blocking file IO and sets process environment variables.
pub fn reload() -> EnvFileReport {
    load_process_env()
}

fn load_process_env() -> EnvFileReport {
    let cwd = std::env::current_dir().ok();
    let report = load_from(
        cwd.as_deref(),
        resolve_aura_home,
        // Never overrides: `load_from` only calls `set` for unset names.
        |name, value| std::env::set_var(name, value),
        |name| std::env::var_os(name).is_some(),
    );
    log_new_warnings(&report);
    report
}

/// `AURA_HOME` from the process environment, read after the working
/// directory's `.env` has been applied.
fn resolve_aura_home() -> Result<Option<PathBuf>, AuraHomeError> {
    match AuraHome::resolve() {
        Ok(home) => Ok(Some(home.path)),
        Err(AuraHomeError::NoHome) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Log each distinct warning once per process, however many times the
/// files are reloaded.
fn log_new_warnings(report: &EnvFileReport) {
    static LOGGED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let logged = LOGGED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut logged = logged.lock().unwrap_or_else(PoisonError::into_inner);
    for warning in &report.warnings {
        if logged.insert(warning.clone()) {
            tracing::warn!("{warning}");
        }
    }
}

/// The loader with its environment and filesystem roots passed in, so
/// tests neither touch the process environment nor the real `$HOME`.
///
/// `aura_home` is called after the working directory's `.env` has been
/// applied. `set` is only called for names `is_set` reports unset.
pub(crate) fn load_from(
    cwd: Option<&Path>,
    aura_home: impl FnOnce() -> Result<Option<PathBuf>, AuraHomeError>,
    mut set: impl FnMut(&str, &str),
    is_set: impl Fn(&str) -> bool,
) -> EnvFileReport {
    let mut report = EnvFileReport::default();

    let project_file = cwd.and_then(find_upward);
    if let Some(path) = &project_file {
        load_file(path, &mut set, &is_set, &mut report);
    }

    match aura_home() {
        Ok(Some(home)) => {
            let path = home.join(ENV_FILE_NAME);
            let is_duplicate = project_file
                .as_deref()
                .is_some_and(|project| same_file(project, &path));
            if !is_duplicate && path.is_file() {
                load_file(&path, &mut set, &is_set, &mut report);
            }
        }
        Ok(None) => {}
        Err(err) => report
            .warnings
            .push(format!("skipped AURA_HOME/.env: {err}")),
    }

    report
}

/// The nearest `.env` in `start` or one of its parents.
fn find_upward(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join(ENV_FILE_NAME))
        .find(|candidate| candidate.is_file())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Parse one file and apply its unset variables. Stops at the first
/// parse error, as `dotenvy` does, keeping the lines before it.
fn load_file(
    path: &Path,
    set: &mut impl FnMut(&str, &str),
    is_set: &impl Fn(&str) -> bool,
    report: &mut EnvFileReport,
) {
    let contents = match read_bounded(path) {
        Ok(contents) => contents,
        Err(warning) => {
            report.warnings.push(warning);
            return;
        }
    };
    // A UTF-8 byte order mark would otherwise become part of the first
    // name.
    let text = contents.strip_prefix('\u{feff}').unwrap_or(&contents);

    let mut holds_api_key = false;
    for item in dotenvy::from_read_iter(text.as_bytes()) {
        let (name, value) = match item {
            Ok(pair) => pair,
            Err(err) => {
                report.warnings.push(parse_warning(path, &err));
                break;
            }
        };
        let value = Zeroizing::new(value);
        if name == BOXD_API_KEY && !value.trim().is_empty() {
            holds_api_key = true;
        }
        if !is_set(&name) {
            set(&name, &value);
        }
    }

    report.loaded.push(path.to_path_buf());
    if holds_api_key {
        if let Some(warning) = permission_warning(path) {
            report.warnings.push(warning);
        }
    }
}

fn read_bounded(path: &Path) -> Result<Zeroizing<String>, String> {
    let display = path.display();
    let metadata =
        std::fs::metadata(path).map_err(|err| format!("could not read {display}: {err}"))?;
    if metadata.len() > MAX_ENV_FILE_BYTES {
        return Err(format!(
            "skipped {display}: larger than {MAX_ENV_FILE_BYTES} bytes"
        ));
    }
    std::fs::read_to_string(path)
        .map(Zeroizing::new)
        .map_err(|err| format!("could not read {display}: {err}"))
}

/// A parse warning that names the file and position but never quotes
/// the line, which may hold a secret.
fn parse_warning(path: &Path, err: &dotenvy::Error) -> String {
    let display = path.display();
    match err {
        dotenvy::Error::LineParse(_, index) => {
            format!("could not parse {display} (error at line index {index}); later lines skipped")
        }
        dotenvy::Error::Io(io) => format!("could not read {display}: {io}"),
        _ => format!("could not parse {display}; later lines skipped"),
    }
}

#[cfg(unix)]
fn permission_warning(path: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(path).ok()?.permissions().mode();
    (mode & 0o044 != 0).then(|| {
        let display = path.display();
        format!(
            "{display} holds {BOXD_API_KEY} and other users can read it; run `chmod 600 {display}`"
        )
    })
}

#[cfg(not(unix))]
fn permission_warning(_path: &Path) -> Option<String> {
    None
}

#[cfg(test)]
mod tests;
