//! Windows program lookup: `PATHEXT`-aware resolution and a PATH merged
//! from the registry, so `.cmd` / `.bat` shims run without `cmd.exe`.

/// PATHEXT-aware program resolution for [`cmd_spawn`](super::cmd_spawn)
/// on Windows.
///
/// Rust's standard library performs a bare-name PATH search that ignores
/// `PATHEXT`, so `Command::new("npm")` cannot find `npm.cmd`. This helper
/// uses [`which::which_in`] (which *does* honour `PATHEXT`) against the
/// freshly-merged registry+process PATH to produce an absolute path
/// pointing at the correct shim, while leaving caller-supplied paths
/// (anything with a separator) and explicit-extension names (`foo.exe`,
/// `bar.cmd`) untouched so we don't second-guess the operator's intent.
///
/// Returns `None` when:
/// - the input already contains a path separator (so it's a relative or
///   absolute path the OS can resolve directly);
/// - the input already ends with a recognised executable extension;
/// - `which` cannot find any matching binary on PATH (the caller then
///   falls back to `Command::new(program)` and lets the OS produce its
///   native `program not found` error).
pub(super) fn windows_resolve_program(
    program: &str,
    fresh_path: Option<&str>,
    cwd: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(program);
    if path.components().count() > 1 {
        return None;
    }
    let lower = program.to_ascii_lowercase();
    if lower.ends_with(".exe")
        || lower.ends_with(".cmd")
        || lower.ends_with(".bat")
        || lower.ends_with(".com")
        || lower.ends_with(".ps1")
    {
        return None;
    }
    match fresh_path {
        Some(p) => which::which_in(program, Some(p), cwd).ok(),
        None => which::which(program).ok(),
    }
}

/// Read the current Machine + User PATH from the Windows registry and merge it
/// with the process PATH so that both registry entries (which may have been
/// updated since the harness started) and session-only entries (e.g. Python
/// user-scripts installed via pip) are available to child processes.
pub(super) fn refresh_system_path() -> Option<String> {
    fn read_reg_path(key: &str) -> Option<String> {
        std::process::Command::new("reg")
            .args(["query", key, "/v", "Path"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                s.lines()
                    .find(|l| l.contains("REG_"))
                    .and_then(|l| {
                        l.split("REG_EXPAND_SZ")
                            .nth(1)
                            .or_else(|| l.split("REG_SZ").nth(1))
                    })
                    .map(|v| v.trim().to_string())
            })
            .map(|p| expand_env_vars(&p))
    }

    let machine_reg =
        read_reg_path(r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment");
    let user_reg = read_reg_path(r"HKCU\Environment");

    let process_path = std::env::var("PATH").unwrap_or_default();

    let mut segments: Vec<&str> = Vec::new();

    if let Some(ref m) = machine_reg {
        segments.extend(m.split(';').filter(|s| !s.is_empty()));
    }
    if let Some(ref u) = user_reg {
        segments.extend(u.split(';').filter(|s| !s.is_empty()));
    }
    for entry in process_path.split(';').filter(|s| !s.is_empty()) {
        if !segments
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(entry))
        {
            segments.push(entry);
        }
    }

    if segments.is_empty() {
        return None;
    }

    Some(segments.join(";"))
}

/// Expand `%VAR%` patterns in a string using the current process environment.
/// Registry PATH values are stored as REG_EXPAND_SZ with variables like
/// `%SystemRoot%`, `%USERPROFILE%`, etc. that must be resolved before use.
fn expand_env_vars(input: &str) -> String {
    let mut result = input.to_string();
    while let Some(start) = result.find('%') {
        if let Some(end) = result[start + 1..].find('%') {
            let var_name = &result[start + 1..start + 1 + end];
            let replacement = std::env::var(var_name).unwrap_or_default();
            result = format!(
                "{}{}{}",
                &result[..start],
                replacement,
                &result[start + 1 + end + 1..]
            );
        } else {
            break;
        }
    }
    result
}
