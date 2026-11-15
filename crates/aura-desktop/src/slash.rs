//! Slash commands typed in the chat input.

pub struct SlashCommand {
    pub name: &'static str,
    pub args: &'static str,
    pub about: &'static str,
}

pub const COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "help",
        args: "",
        about: "Commands and shortcuts",
    },
    SlashCommand {
        name: "model",
        args: "[id]",
        about: "Pick the model for the next turn",
    },
    SlashCommand {
        name: "new",
        args: "",
        about: "Start a new session (reset context)",
    },
    SlashCommand {
        name: "clear",
        args: "",
        about: "Clear the conversation on screen",
    },
    SlashCommand {
        name: "agents",
        args: "",
        about: "Show projects and permanent agents",
    },
    SlashCommand {
        name: "agent",
        args: "new [name] | <n|name>",
        about: "Create or switch agents",
    },
    SlashCommand {
        name: "spawn",
        args: "<permanent agent>",
        about: "Start an instance of a permanent agent",
    },
    SlashCommand {
        name: "project",
        args: "",
        about: "Create a project in a folder",
    },
    SlashCommand {
        name: "name",
        args: "[project name]",
        about: "Rename the current project",
    },
    SlashCommand {
        name: "records",
        args: "",
        about: "Show the kernel record",
    },
    SlashCommand {
        name: "processes",
        args: "",
        about: "Show running and recent work",
    },
    SlashCommand {
        name: "files",
        args: "",
        about: "Show the sandbox file tree",
    },
    SlashCommand {
        name: "terminal",
        args: "",
        about: "Switch the input between chat and shell",
    },
    SlashCommand {
        name: "expand",
        args: "",
        about: "Expand or collapse every step",
    },
    SlashCommand {
        name: "login",
        args: "[zos|claude|chatgpt] [paste]",
        about: "Sign in or link a plan",
    },
    SlashCommand {
        name: "logout",
        args: "[zos|claude|chatgpt]",
        about: "Sign out or unlink a plan",
    },
    SlashCommand {
        name: "whoami",
        args: "",
        about: "Show sign-in state",
    },
    SlashCommand {
        name: "status",
        args: "",
        about: "Show session status",
    },
    SlashCommand {
        name: "history",
        args: "",
        about: "Show recent history",
    },
    SlashCommand {
        name: "quit",
        args: "",
        about: "Quit AURA",
    },
];

/// Commands whose name starts with what was typed after `/`, while the
/// input is still a single word.
pub fn matching(input: &str) -> Vec<&'static SlashCommand> {
    let Some(word) = input.strip_prefix('/') else {
        return Vec::new();
    };
    if word.contains(char::is_whitespace) {
        return Vec::new();
    }
    let word = word.to_ascii_lowercase();
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(&word))
        .collect()
}

/// Canonical plan id for `/login` and `/logout` arguments.
pub fn plan_provider(arg: &str) -> Option<&'static str> {
    match arg.trim().to_ascii_lowercase().as_str() {
        "claude" | "anthropic" => Some("claude"),
        "chatgpt" | "openai" => Some("chatgpt"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_by_prefix_until_a_space() {
        let names: Vec<_> = matching("/mo").iter().map(|c| c.name).collect();
        assert_eq!(names, ["model"]);
        assert!(matching("/model x").is_empty());
        assert!(matching("hello").is_empty());
        assert_eq!(matching("/").len(), COMMANDS.len());
    }

    #[test]
    fn plan_aliases() {
        assert_eq!(plan_provider("OpenAI"), Some("chatgpt"));
        assert_eq!(plan_provider("zos"), None);
    }
}
