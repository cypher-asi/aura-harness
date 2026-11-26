//! The git and `gh` settings files a claim sets up.

use super::*;

/// Where git and `gh` read the settings [`write_git_files`] writes,
/// under a home folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFiles {
    /// git's own settings (`~/.config/git/config`).
    pub config: PathBuf,
    /// The credential store git's helper reads.
    pub credentials: PathBuf,
    /// `gh`'s hosts file.
    pub gh_hosts: PathBuf,
}

impl GitFiles {
    /// The files under `home`.
    #[must_use]
    pub fn under(home: &Path) -> Self {
        let config = home.join(".config");
        Self {
            config: config.join("git").join("config"),
            credentials: config.join("aura").join("git-credentials"),
            gh_hosts: config.join("gh").join("hosts.yml"),
        }
    }
}

/// Whether `value` can sit in a git config value unquoted.
fn plain_config_value(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| !c.is_control() && !matches!(c, '"' | '\\' | '#' | ';'))
}

/// Write a file only the machine's user can read, replacing it whole.
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other(format!("{} has no folder", path.display())))?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    let tmp = path.with_extension("aura-tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    {
        use std::io::Write;
        let mut file = options.open(&tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Set up git and `gh` for `git` (a claim's, its placeholder filled):
/// the bot as commit author, `git push` pushing the current branch, and,
/// when the claim has a token, the placeholder as the password git sends
/// to `github.com` and the token `gh` sends to its API. Each run replaces
/// the files whole; a claim without a token removes the token files.
/// Blocking IO; spawns nothing.
///
/// # Errors
///
/// When a file can't be written, or the placeholder is not one.
pub fn write_git_files(home: &Path, git: &GitClaim) -> std::io::Result<()> {
    let files = GitFiles::under(home);
    let name = git
        .author_name
        .as_deref()
        .filter(|name| plain_config_value(name))
        .unwrap_or("aura-agent");
    let email = git
        .author_email
        .as_deref()
        .filter(|email| plain_config_value(email))
        .unwrap_or("aura-agent@localhost");
    let mut config =
        format!("[user]\n\tname = {name}\n\temail = {email}\n[push]\n\tdefault = current\n");
    match git.placeholder.as_deref() {
        Some(placeholder) => {
            config.push_str(&write_token_files(&files, placeholder, name)?);
        }
        None => remove_token_files(&files)?,
    }
    write_private(&files.config, &config)
}

/// Write the credential store and `gh`'s hosts file for `placeholder`;
/// returns the git config section that points git at the store.
fn write_token_files(files: &GitFiles, placeholder: &str, name: &str) -> std::io::Result<String> {
    if !is_placeholder(placeholder) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the git token is not a boxd placeholder",
        ));
    }
    let store = files.credentials.to_string_lossy().into_owned();
    if !store
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'))
    {
        return Err(std::io::Error::other(format!(
            "{store} can't be named in a credential helper"
        )));
    }
    write_private(
        &files.credentials,
        &format!(
            "https://x-access-token:{}@github.com\n",
            url_encoded(placeholder)
        ),
    )?;
    write_private(
        &files.gh_hosts,
        &format!(
            "github.com:\n    oauth_token: {}\n    user: {}\n    git_protocol: https\n",
            yaml_quoted(placeholder),
            yaml_quoted(name)
        ),
    )?;
    // The empty helper drops any helper set elsewhere for GitHub.
    Ok(format!(
        "[credential \"https://github.com\"]\n\thelper =\n\thelper = store --file {store}\n"
    ))
}

/// Remove the token files a claim without a token leaves stale.
fn remove_token_files(files: &GitFiles) -> std::io::Result<()> {
    for stale in [&files.credentials, &files.gh_hosts] {
        match std::fs::remove_file(stale) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}
