//! Bash-style `[user@host dir]$ ` prompt shown before the input line.

use std::path::Path;

/// Build the prompt for the current user, host and working directory.
pub(super) fn detect() -> String {
    let cwd = std::env::current_dir().ok();
    let dir = cwd.as_deref().map_or_else(
        || "?".to_string(),
        |cwd| dir_label(cwd, dirs::home_dir().as_deref()),
    );
    format_prompt(&user(), &host(), &dir)
}

fn format_prompt(user: &str, host: &str, dir: &str) -> String {
    format!("[{user}@{host} {dir}]$ ")
}

fn user() -> String {
    non_empty_env("USER")
        .or_else(|| non_empty_env("USERNAME"))
        .unwrap_or_else(|| "user".to_string())
}

/// Short host name, like bash's `\h`.
fn host() -> String {
    let full = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| non_empty_env("HOSTNAME"))
        .or_else(|| non_empty_env("COMPUTERNAME"))
        .unwrap_or_else(|| "localhost".to_string());
    short_host(&full).to_string()
}

fn short_host(full: &str) -> &str {
    full.split('.').next().unwrap_or(full)
}

/// Last path component, or `~` for the home directory, like bash's `\W`.
fn dir_label(cwd: &Path, home: Option<&Path>) -> String {
    if home == Some(cwd) {
        return "~".to_string();
    }
    cwd.file_name().map_or_else(
        || cwd.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{dir_label, format_prompt, short_host};
    use std::path::Path;

    #[test]
    fn formats_like_bash() {
        assert_eq!(
            format_prompt("n3o", "arch-aura", "aura-harness"),
            "[n3o@arch-aura aura-harness]$ "
        );
    }

    #[test]
    fn host_drops_domain() {
        assert_eq!(short_host("arch-aura.lan"), "arch-aura");
        assert_eq!(short_host("arch-aura"), "arch-aura");
    }

    #[test]
    fn dir_is_basename_or_tilde() {
        let home = Path::new("/home/n3o");
        assert_eq!(
            dir_label(Path::new("/home/n3o/code/aura-harness"), Some(home)),
            "aura-harness"
        );
        assert_eq!(dir_label(home, Some(home)), "~");
        assert_eq!(dir_label(Path::new("/"), Some(home)), "/");
    }
}
