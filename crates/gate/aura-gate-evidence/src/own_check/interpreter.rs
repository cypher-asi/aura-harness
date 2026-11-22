//! Whether a command starts an interpreter, which runs the code or script
//! it is given.

/// Programs that run the code or script they are given.
const INTERPRETERS: &[&str] = &[
    "python", "python3", "node", "deno", "bun", "tsx", "ts-node", "ruby", "perl", "php", "lua",
    "awk", "gawk", "mawk",
];

/// Shells, which run a script, or with `-c` the commands after it.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh"];

/// Programs, and shell words, that run the command after them.
const WRAPPERS: &[&str] = &[
    "env", "time", "nice", "exec", "command", "sudo", "do", "then", "else", "if", "elif", "while",
    "until", "!", "{",
];

/// Programs that run the command after their next word (`timeout 60`,
/// `uv run`).
const WRAPPERS_WITH_A_WORD: &[&str] = &["timeout", "uv", "poetry", "pipenv", "pdm"];

/// Whether a simple command of `command` starts an interpreter, past
/// variable assignments and wrappers (`env`, `timeout 60`, `uv run`), or
/// runs a script by its path (`./backup.sh`). In `sh -c …` it is the
/// commands after `-c` that run.
pub(super) fn starts_interpreter(command: &str) -> bool {
    command.split(['&', ';', '|', '\n', '(']).any(|part| {
        let mut words = part
            .split_whitespace()
            .map(|word| word.trim_matches(['\'', '"']));
        while let Some(word) = words.next() {
            if (word.contains('=') && !word.starts_with('-')) || WRAPPERS.contains(&word) {
                continue;
            }
            if WRAPPERS_WITH_A_WORD.contains(&word) {
                words.next();
                continue;
            }
            let program = word.rsplit('/').next().unwrap_or(word);
            if SHELLS.contains(&program) {
                let mut after = words.clone();
                if after.next().is_some_and(|flag| {
                    flag.starts_with('-') && !flag.starts_with("--") && flag.contains('c')
                }) {
                    words = after;
                    continue;
                }
                return true;
            }
            return is_interpreter(program) || (word.contains('/') && program.contains('.'));
        }
        false
    })
}

fn is_interpreter(program: &str) -> bool {
    INTERPRETERS.contains(&program)
        || program
            .strip_prefix("python")
            .is_some_and(|version| version.chars().all(|c| c.is_ascii_digit() || c == '.'))
}
