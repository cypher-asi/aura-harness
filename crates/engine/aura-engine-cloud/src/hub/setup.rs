//! `aura cloud setup`: get a boxd API key into `AURA_HOME/.env`.
//!
//! 1. Find the `boxd` CLI (on `PATH`, or `~/.local/bin/boxd` where its
//!    installer puts it). Without it, say how to install it.
//! 2. Sign in with `boxd auth login` (browser) unless `boxd auth` says
//!    a login is there.
//! 3. Mint a key with `boxd auth keys create aura-<host>`; the raw value
//!    comes on stdout and is read from the pipe, never shown.
//! 4. Write it to `AURA_HOME/.env` as `BOXD_API_KEY`, keeping every
//!    other line, with mode 0600 on Unix.
//!
//! The installer itself (`curl -fsSL https://boxd.sh/downloads/install.sh
//! | sh`) is only ever run after the user confirms, and without `--yes`,
//! so only the binary is installed.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use aura_config::BoxdApiKey;
use tokio::process::Command;

/// The boxd CLI installer for macOS and Linux.
pub const INSTALL_COMMAND: &str = "curl -fsSL https://boxd.sh/downloads/install.sh | sh";
/// The boxd CLI installer for Windows (PowerShell).
pub const INSTALL_COMMAND_WINDOWS: &str = "irm https://boxd.sh/downloads/install.ps1 | iex";

/// Limit on `boxd` calls that need no person.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Limit on the browser sign-in.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// What went wrong during setup.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    /// The `boxd` CLI is not installed.
    #[error(
        "the boxd CLI is not installed; install it with `{}` and run setup again",
        install_command()
    )]
    NoCli,
    /// A `boxd` command failed.
    #[error("`boxd {command}` failed: {message}")]
    Boxd {
        /// The subcommand.
        command: String,
        /// Its stderr, trimmed.
        message: String,
    },
    /// The minted key does not look like a boxd key.
    #[error("boxd printed something that is not an API key")]
    BadKey,
    /// Writing the `.env` file failed.
    #[error("could not write {path}: {source}")]
    Write {
        /// The file.
        path: PathBuf,
        /// The error.
        #[source]
        source: std::io::Error,
    },
}

/// The installer for this OS.
#[must_use]
pub const fn install_command() -> &'static str {
    if cfg!(windows) {
        INSTALL_COMMAND_WINDOWS
    } else {
        INSTALL_COMMAND
    }
}

/// The `boxd` binary: on `PATH`, or where its installer puts it.
#[must_use]
pub fn find_boxd(home: Option<&Path>) -> Option<PathBuf> {
    which::which("boxd").ok().or_else(|| {
        let name = if cfg!(windows) { "boxd.exe" } else { "boxd" };
        home.map(|home| home.join(".local").join("bin").join(name))
            .filter(|path| path.is_file())
    })
}

async fn run_boxd(
    boxd: &Path,
    args: &[&str],
    timeout: Duration,
    interactive: bool,
) -> Result<String, SetupError> {
    let mut command = Command::new(boxd);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if interactive {
        // The browser sign-in prints its link and code on stderr.
        command.stderr(Stdio::inherit()).stdin(Stdio::inherit());
    }
    let joined = args.join(" ");
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| SetupError::Boxd {
            command: joined.clone(),
            message: format!("timed out after {timeout:?}"),
        })?
        .map_err(|err| SetupError::Boxd {
            command: joined.clone(),
            message: err.to_string(),
        })?;
    if !output.status.success() {
        return Err(SetupError::Boxd {
            command: joined,
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Whether `boxd auth` finds a login.
pub async fn signed_in(boxd: &Path) -> bool {
    run_boxd(boxd, &["auth"], CALL_TIMEOUT, false).await.is_ok()
}

/// Sign in through the browser.
///
/// # Errors
///
/// When `boxd auth login` fails or times out.
pub async fn sign_in(boxd: &Path) -> Result<(), SetupError> {
    run_boxd(boxd, &["auth", "login"], LOGIN_TIMEOUT, true)
        .await
        .map(|_| ())
}

/// Mint a key called `name`.
///
/// # Errors
///
/// When `boxd auth keys create` fails or prints no key.
pub async fn mint_key(boxd: &Path, name: &str) -> Result<BoxdApiKey, SetupError> {
    let stdout = zeroize::Zeroizing::new(
        run_boxd(boxd, &["auth", "keys", "create", name], CALL_TIMEOUT, false).await?,
    );
    let key = stdout
        .split_whitespace()
        .find(|word| word.starts_with(aura_config::cloud::BOXD_API_KEY_PREFIX))
        .ok_or(SetupError::BadKey)?;
    Ok(BoxdApiKey::new(key))
}

/// `contents` of a `.env` file with `BOXD_API_KEY` set to `key`:
/// an existing line (commented or not) is replaced, else one is added.
#[must_use]
pub fn with_key(contents: &str, key: &BoxdApiKey) -> String {
    let line = format!("BOXD_API_KEY={}", key.expose());
    let mut replaced = false;
    let mut out: Vec<String> = contents
        .lines()
        .map(|existing| {
            let bare = existing.trim_start().trim_start_matches('#').trim_start();
            let bare = bare.strip_prefix("export ").unwrap_or(bare);
            if !replaced && bare.starts_with("BOXD_API_KEY=") {
                replaced = true;
                line.clone()
            } else {
                existing.to_string()
            }
        })
        .collect();
    if !replaced {
        if out.last().is_some_and(|last| !last.is_empty()) {
            out.push(String::new());
        }
        out.push("# boxd API key for cloud agents (written by aura cloud setup)".to_string());
        out.push(line);
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Write `key` into `env_file`, keeping its other lines, mode 0600.
///
/// # Errors
///
/// When the file can't be read or written.
pub async fn store_key(env_file: &Path, key: &BoxdApiKey) -> Result<(), SetupError> {
    let write = |source| SetupError::Write {
        path: env_file.to_path_buf(),
        source,
    };
    if let Some(dir) = env_file.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(write)?;
    }
    let existing = match tokio::fs::read_to_string(env_file).await {
        Ok(text) => zeroize::Zeroizing::new(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            zeroize::Zeroizing::new(String::new())
        }
        Err(err) => return Err(write(err)),
    };
    let updated = zeroize::Zeroizing::new(with_key(&existing, key));
    let tmp = env_file.with_extension("env.tmp");
    tokio::fs::write(&tmp, updated.as_bytes())
        .await
        .map_err(write)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(write)?;
    }
    tokio::fs::rename(&tmp, env_file).await.map_err(write)
}

/// The key name for this machine: `aura-<hostname>`.
#[must_use]
pub fn key_name() -> String {
    let host = sysinfo::System::host_name().unwrap_or_else(|| "host".to_string());
    format!("aura-{}", crate::names::dns_fragment(&host))
}

/// Run the whole setup: find the CLI, sign in if needed, mint a key and
/// store it in `env_file`. Returns the redacted key.
///
/// # Errors
///
/// [`SetupError::NoCli`] without the CLI; otherwise the failing step.
pub async fn run(home: Option<&Path>, env_file: &Path) -> Result<BoxdApiKey, SetupError> {
    let boxd = find_boxd(home).ok_or(SetupError::NoCli)?;
    if !signed_in(&boxd).await {
        sign_in(&boxd).await?;
    }
    let key = mint_key(&boxd, &key_name()).await?;
    store_key(env_file, &key).await?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "bxd_0123456789abcdefghijABCDEFGHIJ0123456789xyz";

    #[test]
    fn a_key_line_is_added_or_replaced() {
        let key = BoxdApiKey::new(KEY);
        let added = with_key("AURA_LOCAL_MODEL=qwen3:4b\n", &key);
        assert!(added.starts_with("AURA_LOCAL_MODEL=qwen3:4b\n"));
        assert!(added.ends_with(&format!("BOXD_API_KEY={KEY}\n")));
        let replaced = with_key("# BOXD_API_KEY=bxd_...\nX=1\n", &key);
        assert_eq!(replaced, format!("BOXD_API_KEY={KEY}\nX=1\n"));
        let exported = with_key("export BOXD_API_KEY=old\n", &key);
        assert_eq!(exported, format!("BOXD_API_KEY={KEY}\n"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn setup_mints_a_key_with_a_fake_cli_and_stores_it_privately() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join(".local/bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let fake = bin.join("boxd");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\ncase \"$1 $2 $3\" in\n  'auth  ') exit 0 ;;\n  'auth keys create') echo '{KEY}'; echo 'id: k1' >&2 ;;\n  *) exit 3 ;;\nesac\n"
            ),
        )
        .expect("write fake");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let env_file = dir.path().join("aura/.env");
        std::fs::create_dir_all(env_file.parent().expect("parent")).expect("dir");
        std::fs::write(&env_file, "AURA_LOCAL_MODEL=qwen3:4b\n").expect("seed");

        let boxd = find_boxd(Some(dir.path())).expect("found in ~/.local/bin");
        assert!(signed_in(&boxd).await);
        let key = mint_key(&boxd, "aura-test").await.expect("minted");
        assert_eq!(key.expose(), KEY);
        assert!(!key.to_string().contains("0123456789abcdef"));
        store_key(&env_file, &key).await.expect("stored");

        let text = std::fs::read_to_string(&env_file).expect("read");
        assert!(text.contains("AURA_LOCAL_MODEL=qwen3:4b"));
        assert!(text.contains(&format!("BOXD_API_KEY={KEY}")));
        let mode = std::fs::metadata(&env_file)
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn without_a_cli_the_error_names_the_installer() {
        let text = SetupError::NoCli.to_string();
        assert!(text.contains("boxd.sh/downloads/install"), "{text}");
    }
}
