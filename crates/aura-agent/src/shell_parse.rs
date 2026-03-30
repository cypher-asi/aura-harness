//! Shell command extraction from task titles and descriptions.

/// Remove trailing natural-language prose from an extracted shell command.
/// e.g. "cargo build --workspace to confirm compilation" -> "cargo build --workspace"
pub fn trim_prose_suffix(cmd: &str) -> String {
    let lower = cmd.to_lowercase();
    let boundaries = [
        " in order to ",
        " and verify ",
        " and confirm ",
        " and check ",
        " so that ",
        " to confirm ",
        " to verify ",
        " to check ",
        " to ensure ",
        " to produce ",
        " to install ",
        " to run ",
        " to test ",
        " to build ",
        " to see ",
        " to make ",
        " for verification",
        " for testing",
        " in a shell",
        " in the shell",
        " in the project",
        " in order ",
        " to ",
        " which ",
        " that ",
        " after ",
        " before ",
        " since ",
        " because ",
        " if ",
    ];

    let mut best_cut = cmd.len();
    for boundary in &boundaries {
        if let Some(pos) = lower.find(boundary) {
            if pos > 0 && pos < best_cut {
                let before = cmd[..pos].trim();
                if !before.is_empty() {
                    best_cut = pos;
                    break;
                }
            }
        }
    }

    cmd[..best_cut].trim().to_string()
}

/// Extract a shell command from backtick-delimited text in the description.
/// Looks for `command` patterns where the command starts with a known indicator.
pub fn extract_backtick_command(text: &str, shell_indicators: &[&str]) -> Option<String> {
    let mut search_from = 0;
    while let Some(start) = text[search_from..].find('`') {
        let abs_start = search_from + start + 1;
        if abs_start >= text.len() {
            break;
        }
        if let Some(end) = text[abs_start..].find('`') {
            let candidate = text[abs_start..abs_start + end].trim();
            if !candidate.is_empty()
                && shell_indicators
                    .iter()
                    .any(|ind| candidate.to_lowercase().starts_with(ind))
            {
                return Some(candidate.to_string());
            }
            search_from = abs_start + end + 1;
        } else {
            break;
        }
    }
    None
}

/// Extract a command from prose like "Execute cd ui && npm install in a shell with..."
/// Looks for a prefix ("execute ", "run ") followed by a shell indicator, and cuts
/// the command at natural language boundaries (" in a shell", " in a ", " to ", " with ").
pub fn extract_prose_command(
    text: &str,
    prefixes: &[&str],
    shell_indicators: &[&str],
) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim().to_lowercase();
        for prefix in prefixes {
            if !trimmed.starts_with(prefix) {
                continue;
            }
            let after_prefix = &line.trim()[prefix.len()..];
            if !shell_indicators
                .iter()
                .any(|ind| after_prefix.to_lowercase().starts_with(ind))
            {
                continue;
            }
            let boundaries = [" in a shell", " in a ", " to ", " with "];
            let mut end = after_prefix.len();
            for boundary in &boundaries {
                if let Some(pos) = after_prefix.to_lowercase().find(boundary) {
                    if pos < end && pos > 0 {
                        end = pos;
                    }
                }
            }
            let cmd = after_prefix[..end].trim();
            if !cmd.is_empty() {
                return Some(cmd.to_string());
            }
        }
    }
    None
}

const SHELL_PREFIXES: &[&str] = &["run ", "execute ", "run: "];

const SHELL_INDICATORS: &[&str] = &[
    "cd ",
    "npm ",
    "npx ",
    "cargo ",
    "yarn ",
    "pnpm ",
    "pip ",
    "python ",
    "node ",
    "sh ",
    "bash ",
    "powershell ",
    "cmd ",
    "make ",
    "gradle ",
    "mvn ",
    "dotnet ",
    "go ",
    "rustup ",
    "apt ",
    "brew ",
];

/// Attempt to extract a runnable shell command from a task's title and description.
///
/// Returns `None` when the task does not appear to be a simple shell command.
pub fn extract_shell_command(title: &str, description: &str) -> Option<String> {
    let title = title.trim();
    let desc = description.trim();

    if let Some(cmd) = extract_backtick_command(desc, SHELL_INDICATORS) {
        return Some(cmd);
    }

    if let Some(cmd) = extract_prose_command(desc, SHELL_PREFIXES, SHELL_INDICATORS) {
        return Some(cmd);
    }

    let candidate = if SHELL_PREFIXES
        .iter()
        .any(|p| title.to_lowercase().starts_with(p))
    {
        let lower = title.to_lowercase();
        let cmd_start = SHELL_PREFIXES
            .iter()
            .find_map(|p| {
                if lower.starts_with(p) {
                    Some(p.len())
                } else {
                    None
                }
            })
            .unwrap_or(0);
        title[cmd_start..].trim().to_string()
    } else if SHELL_INDICATORS
        .iter()
        .any(|ind| title.to_lowercase().starts_with(ind))
    {
        title.to_string()
    } else if !desc.is_empty() && desc.lines().count() <= 2 {
        let first_line = desc.lines().next().unwrap_or("").trim();
        if SHELL_INDICATORS
            .iter()
            .any(|ind| first_line.to_lowercase().starts_with(ind))
        {
            first_line.to_string()
        } else {
            return None;
        }
    } else {
        return None;
    };

    if candidate.is_empty() {
        None
    } else {
        Some(trim_prose_suffix(&candidate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_prose_no_suffix() {
        assert_eq!(
            trim_prose_suffix("cargo build --workspace"),
            "cargo build --workspace"
        );
    }

    #[test]
    fn trim_prose_to_confirm() {
        assert_eq!(
            trim_prose_suffix("cargo build --workspace to confirm compilation"),
            "cargo build --workspace"
        );
    }

    #[test]
    fn trim_prose_in_order_to() {
        assert_eq!(
            trim_prose_suffix("npm install in order to set up deps"),
            "npm install"
        );
    }

    #[test]
    fn extract_backtick_npm() {
        let indicators = &["npm ", "cargo "];
        let text = "Please run `npm install` in the project";
        assert_eq!(
            extract_backtick_command(text, indicators),
            Some("npm install".into())
        );
    }

    #[test]
    fn extract_backtick_ignores_non_shell() {
        let indicators = &["npm ", "cargo "];
        let text = "The variable `count` should be incremented";
        assert_eq!(extract_backtick_command(text, indicators), None);
    }

    #[test]
    fn extract_backtick_multiple_picks_first_shell() {
        let indicators = &["npm ", "cargo "];
        let text = "After setting `DEBUG=true`, run `cargo test` to verify";
        assert_eq!(
            extract_backtick_command(text, indicators),
            Some("cargo test".into())
        );
    }

    #[test]
    fn extract_prose_run_prefix() {
        let prefixes = &["run ", "execute "];
        let indicators = &["npm ", "cargo "];
        let text = "Run npm install in a shell to set up the project";
        assert_eq!(
            extract_prose_command(text, prefixes, indicators),
            Some("npm install".into())
        );
    }

    #[test]
    fn extract_prose_execute_prefix() {
        let prefixes = &["run ", "execute "];
        let indicators = &["cargo ", "npm "];
        let text = "Execute cargo build --release in a shell";
        assert_eq!(
            extract_prose_command(text, prefixes, indicators),
            Some("cargo build --release".into())
        );
    }

    #[test]
    fn extract_prose_no_match() {
        let prefixes = &["run ", "execute "];
        let indicators = &["npm ", "cargo "];
        assert_eq!(
            extract_prose_command("Implement the feature", prefixes, indicators),
            None
        );
    }

    #[test]
    fn shell_command_from_title_with_run_prefix() {
        assert_eq!(
            extract_shell_command("Run cargo build --workspace", ""),
            Some("cargo build --workspace".into())
        );
    }

    #[test]
    fn shell_command_from_title_indicator_start() {
        assert_eq!(
            extract_shell_command("cargo test --release", "something unrelated"),
            Some("cargo test --release".into())
        );
    }

    #[test]
    fn shell_command_from_description_backtick() {
        assert_eq!(
            extract_shell_command(
                "Set up dependencies",
                "Run `npm install` in the project root",
            ),
            Some("npm install".into())
        );
    }

    #[test]
    fn shell_command_from_description_short_line() {
        assert_eq!(
            extract_shell_command("Install packages", "npm install"),
            Some("npm install".into())
        );
    }

    #[test]
    fn no_shell_command_for_regular_task() {
        assert_eq!(
            extract_shell_command(
                "Implement user auth",
                "Add login and signup endpoints with JWT tokens",
            ),
            None
        );
    }

    #[test]
    fn no_shell_command_for_multiline_desc() {
        assert_eq!(
            extract_shell_command(
                "Add tests",
                "Write unit tests for the auth module.\nCover edge cases for token expiry.",
            )
            .map(|_| ()),
            None
        );
    }
}
