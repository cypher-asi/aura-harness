//! Best-effort system browser launch for sign-in URLs.

use std::process::{Command, Stdio};

/// Ask the OS to open `url`. Returns `false` when no opener could be
/// started; callers always print the URL as well.
#[must_use]
pub fn open_browser(url: &str) -> bool {
    if std::env::var_os("AURA_NO_BROWSER").is_some() {
        return false;
    }
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}
