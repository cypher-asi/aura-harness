//! Which checks of a command line ran with the change stashed away.

use super::command::{parts, runs_check, simple_command, words};

/// Which of a command line's checks ran on the change, and which with
/// the change stashed away: between a `git stash` and the `git stash
/// pop` or `apply` that brings it back, the working tree holds the code
/// as it was before, and a model runs its tests there to see which
/// failures were there already. Each check is kept as its own words
/// ([`check_run`]), so the same check with and without the change reads
/// the same.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Stashing {
    /// The checks run with the change in place
    pub with: Vec<String>,
    /// The checks run with the change stashed away
    pub without: Vec<String>,
    /// Whether the change is stashed away when the command line ends
    pub stashed: bool,
}

impl Stashing {
    /// What `command` runs, the change being `stashed` away or not when
    /// it starts.
    pub(super) fn of(command: &str, mut stashed: bool) -> Self {
        let (mut with, mut without) = (Vec::new(), Vec::new());
        for (part, _) in parts(command) {
            let words = words(part);
            let words = simple_command(&words, true);
            if let ["git", "stash", rest @ ..] = words {
                match rest {
                    ["pop" | "apply", ..] => stashed = false,
                    ["list" | "show" | "drop" | "clear" | "branch" | "create" | "store", ..] => {}
                    _ => stashed = true,
                }
            } else if let Some(run) = check_run(words) {
                if stashed {
                    without.push(run);
                } else {
                    with.push(run);
                }
            }
        }
        Self {
            with,
            without,
            stashed,
        }
    }
}

/// The check `words` (a simple command, as [`simple_command`] leaves it)
/// runs, without where its output is sent; `None` when it runs none.
fn check_run(words: &[&str]) -> Option<String> {
    runs_check(words).then(|| {
        words
            .iter()
            .filter(|word| !word.contains('>'))
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_line_says_which_checks_ran_without_the_change() {
        let ran = Stashing::of(
            "sh .swe/run pytest -q a 2>&1 | tail -2; git stash -q; python -m pytest -q b | \
             grep passed; git stash pop -q; cargo test",
            false,
        );
        assert_eq!(ran.with, ["pytest -q a", "cargo test"]);
        assert_eq!(ran.without, ["pytest -q b"]);
        assert!(!ran.stashed);
        assert!(Stashing::of("git stash", false).stashed);
        assert!(Stashing::of("git stash push -u && pytest", false).stashed);
        assert!(!Stashing::of("git stash list", false).stashed);
        assert!(Stashing::of("git stash show -p", true).stashed);
        assert!(!Stashing::of("git stash apply", true).stashed);
        assert_eq!(Stashing::of("pytest -q", true).without, ["pytest -q"]);
    }
}
