/// Parsed CLI command from user input.
///
/// Input that doesn't start with `/` is treated as a [`Command::Prompt`].
/// Slash-prefixed input is parsed as a named command (e.g. `/status`, `/quit`).
#[derive(Debug)]
pub enum Command {
    /// Free-text prompt sent to the agent for processing.
    Prompt(String),
    /// Display session status (agent ID, sequence, provider).
    Status,
    /// Show the last *n* history entries.
    History(usize),
    /// Approve the current pending tool-execution request.
    Approve,
    /// Deny the current pending tool-execution request.
    Deny,
    /// Show pending file-level changes (diff).
    Diff,
    /// Authenticate with zOS to obtain a JWT for proxy mode.
    Login,
    /// Clear stored zOS credentials and log out.
    Logout,
    /// Show current authentication status.
    Whoami,
    /// Print the help message listing available commands.
    Help,
    /// Exit the CLI gracefully.
    Quit,
    /// Unrecognised slash-command.
    Unknown(String),
}

impl Command {
    pub(crate) fn parse(input: &str) -> Self {
        let input = input.trim();

        if input.is_empty() {
            return Self::Prompt(String::new());
        }

        if !input.starts_with('/') {
            return Self::Prompt(input.to_string());
        }

        let parts: Vec<&str> = input[1..].splitn(2, ' ').collect();
        let cmd = parts[0].to_lowercase();
        let arg = parts.get(1).unwrap_or(&"").trim();

        match cmd.as_str() {
            "status" | "s" => Self::Status,
            "history" | "h" => {
                let n = arg.parse().unwrap_or(10);
                Self::History(n)
            }
            "approve" | "yes" | "y" => Self::Approve,
            "deny" | "no" | "n" => Self::Deny,
            "diff" | "d" => Self::Diff,
            "login" => Self::Login,
            "logout" => Self::Logout,
            "whoami" | "me" => Self::Whoami,
            "help" | "?" => Self::Help,
            "quit" | "exit" | "q" => Self::Quit,
            _ => Self::Unknown(cmd),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_prompt_text() {
        match Command::parse("hello world") {
            Command::Prompt(t) => assert_eq!(t, "hello world"),
            _ => panic!("Expected Prompt"),
        }
    }

    #[test]
    fn test_parse_empty_input() {
        match Command::parse("") {
            Command::Prompt(t) => assert!(t.is_empty()),
            _ => panic!("Expected empty Prompt"),
        }
    }

    #[test]
    fn test_parse_whitespace_only() {
        match Command::parse("   ") {
            Command::Prompt(t) => assert!(t.is_empty()),
            _ => panic!("Expected empty Prompt"),
        }
    }

    #[test]
    fn test_parse_status_commands() {
        assert!(matches!(Command::parse("/status"), Command::Status));
        assert!(matches!(Command::parse("/s"), Command::Status));
        assert!(matches!(Command::parse("/STATUS"), Command::Status));
    }

    #[test]
    fn test_parse_history_default() {
        match Command::parse("/history") {
            Command::History(n) => assert_eq!(n, 10),
            _ => panic!("Expected History"),
        }
    }

    #[test]
    fn test_parse_history_with_arg() {
        match Command::parse("/history 5") {
            Command::History(n) => assert_eq!(n, 5),
            _ => panic!("Expected History"),
        }
    }

    #[test]
    fn test_parse_history_shortcut() {
        match Command::parse("/h 20") {
            Command::History(n) => assert_eq!(n, 20),
            _ => panic!("Expected History"),
        }
    }

    #[test]
    fn test_parse_history_invalid_arg() {
        match Command::parse("/history abc") {
            Command::History(n) => assert_eq!(n, 10),
            _ => panic!("Expected History with default"),
        }
    }

    #[test]
    fn test_parse_approve_commands() {
        assert!(matches!(Command::parse("/approve"), Command::Approve));
        assert!(matches!(Command::parse("/yes"), Command::Approve));
        assert!(matches!(Command::parse("/y"), Command::Approve));
    }

    #[test]
    fn test_parse_deny_commands() {
        assert!(matches!(Command::parse("/deny"), Command::Deny));
        assert!(matches!(Command::parse("/no"), Command::Deny));
        assert!(matches!(Command::parse("/n"), Command::Deny));
    }

    #[test]
    fn test_parse_diff() {
        assert!(matches!(Command::parse("/diff"), Command::Diff));
        assert!(matches!(Command::parse("/d"), Command::Diff));
    }

    #[test]
    fn test_parse_help() {
        assert!(matches!(Command::parse("/help"), Command::Help));
        assert!(matches!(Command::parse("/?"), Command::Help));
    }

    #[test]
    fn test_parse_quit_commands() {
        assert!(matches!(Command::parse("/quit"), Command::Quit));
        assert!(matches!(Command::parse("/exit"), Command::Quit));
        assert!(matches!(Command::parse("/q"), Command::Quit));
    }

    #[test]
    fn test_parse_unknown_command() {
        match Command::parse("/foobar") {
            Command::Unknown(cmd) => assert_eq!(cmd, "foobar"),
            _ => panic!("Expected Unknown"),
        }
    }

    #[test]
    fn test_parse_case_insensitive() {
        assert!(matches!(Command::parse("/QUIT"), Command::Quit));
        assert!(matches!(Command::parse("/Help"), Command::Help));
        assert!(matches!(Command::parse("/DIFF"), Command::Diff));
    }

    #[test]
    fn test_parse_login() {
        assert!(matches!(Command::parse("/login"), Command::Login));
        assert!(matches!(Command::parse("/LOGIN"), Command::Login));
    }

    #[test]
    fn test_parse_logout() {
        assert!(matches!(Command::parse("/logout"), Command::Logout));
        assert!(matches!(Command::parse("/LOGOUT"), Command::Logout));
    }

    #[test]
    fn test_parse_whoami() {
        assert!(matches!(Command::parse("/whoami"), Command::Whoami));
        assert!(matches!(Command::parse("/me"), Command::Whoami));
        assert!(matches!(Command::parse("/WHOAMI"), Command::Whoami));
    }
}
