//! Decide whether submitted input is a shell command or a chat message.

/// Prefix that forces the rest of the line to run as a command.
const FORCE_PREFIX: char = '!';

/// Return the command line to run when `text` is a shell command.
///
/// `!cmd` always runs `cmd`. Unprefixed input runs when its first word
/// is an executable on `PATH` and the line does not read like prose.
pub(super) fn parse(text: &str) -> Option<String> {
    parse_with(text, |program| which::which(program).is_ok())
}

fn parse_with(text: &str, resolves: impl Fn(&str) -> bool) -> Option<String> {
    let text = text.trim();
    if let Some(forced) = text.strip_prefix(FORCE_PREFIX) {
        let forced = forced.trim();
        return (!forced.is_empty()).then(|| forced.to_string());
    }
    let program = text.split_whitespace().next()?;
    if looks_like_prose(text) || !resolves(program) {
        return None;
    }
    Some(text.to_string())
}

/// A question, or three or more plain words with no flags, paths, quotes
/// or operators, is treated as a message to the agent.
fn looks_like_prose(text: &str) -> bool {
    if text.ends_with('?') {
        return true;
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    words.len() >= 3
        && words.iter().all(|word| {
            word.trim_end_matches(['.', ',', '!'])
                .chars()
                .all(|c| c.is_alphabetic() || c == '\'')
        })
}

#[cfg(test)]
mod tests {
    use super::parse_with;

    fn on_path(program: &str) -> bool {
        matches!(program, "ls" | "git" | "make" | "echo" | "cargo")
    }

    fn parse(text: &str) -> Option<String> {
        parse_with(text, on_path)
    }

    #[test]
    fn plain_commands_run() {
        assert_eq!(parse("ls").as_deref(), Some("ls"));
        assert_eq!(parse("ls -la").as_deref(), Some("ls -la"));
        assert_eq!(parse("git status").as_deref(), Some("git status"));
        assert_eq!(parse("ls | head -n 3").as_deref(), Some("ls | head -n 3"));
        assert_eq!(
            parse("cargo test -p aura-terminal").as_deref(),
            Some("cargo test -p aura-terminal")
        );
    }

    #[test]
    fn prefix_forces_command() {
        assert_eq!(
            parse("!echo hi there friend").as_deref(),
            Some("echo hi there friend")
        );
        assert_eq!(
            parse("! unknown-tool --x").as_deref(),
            Some("unknown-tool --x")
        );
        assert_eq!(parse("!"), None);
        assert_eq!(parse("!   "), None);
    }

    #[test]
    fn prose_goes_to_the_agent() {
        assert_eq!(parse("make it faster"), None);
        assert_eq!(parse("ls what is here?"), None);
        assert_eq!(parse("what is this?"), None);
        assert_eq!(parse("hello"), None);
        assert_eq!(parse(""), None);
    }
}
