//! No push leaves `run_command` for a remote: every push from an aura
//! agent goes through the push tools, which put it in the merge queue.
//!
//! Two layers, because a command line can be read but a script it runs
//! can't:
//!
//! 1. [`refuse_raw_push`] refuses, before anything runs, a command whose
//!    words run `git push` (after any of git's global options, anywhere
//!    in a shell script, through `env`, `xargs`, `sh -c` or `eval`) or
//!    `gh pr merge`, or a `gh api` call on a PR's merge
//!    (`repos/<o>/<r>/pulls/<n>/merge`), a write to `/merges` or
//!    `/git/refs`, a GraphQL mutation that merges a PR or moves a ref
//!    (`mergePullRequest`, `enablePullRequestAutoMerge`, `updateRef`, ...),
//!    or the same through `curl` to `api.github.com`. Only the merge
//!    queue merges, as the commit it tested. This is a cheap net, not the
//!    guard: branch protection on the target is. The error points the agent at
//!    `git_ship`.
//! 2. [`PUSH_BLOCK_ENV`], set on every command it starts, rewrites each
//!    network remote's push URL to one git can't push to
//!    (`url.<blocked>.pushInsteadOf`). So a push that a script or a tool
//!    makes fails too, while fetches, clones and every other git command
//!    work as before. The push tools spawn git themselves, without it.
//!
//! Blocking, not detecting and queueing afterwards: a push that already
//! reached the remote can't be taken back, and one to the target branch
//! must never land at all.

use crate::error::ToolError;

/// What a raw push is refused with.
pub(super) const RAW_PUSH_REFUSED: &str = "pushes don't go through run_command: call git_ship \
    (or git_commit_push / git_push) instead. They push your own branch and put it in the merge \
    queue, which merges it into main once the build and tests pass; main itself is never pushed";

/// The push URL every network remote is rewritten to in commands
/// `run_command` starts: no git transport serves it.
#[cfg(test)]
const BLOCKED_PUSH_URL: &str = "aura-blocked-push://use-git_ship/";

/// The environment that makes git push to no network remote: one
/// `pushInsteadOf` per URL form a remote can take.
pub(super) const PUSH_BLOCK_ENV: [(&str, &str); 9] = [
    ("GIT_CONFIG_COUNT", "4"),
    (
        "GIT_CONFIG_KEY_0",
        "url.aura-blocked-push://use-git_ship/.pushInsteadOf",
    ),
    ("GIT_CONFIG_VALUE_0", "https://"),
    (
        "GIT_CONFIG_KEY_1",
        "url.aura-blocked-push://use-git_ship/.pushInsteadOf",
    ),
    ("GIT_CONFIG_VALUE_1", "http://"),
    (
        "GIT_CONFIG_KEY_2",
        "url.aura-blocked-push://use-git_ship/.pushInsteadOf",
    ),
    ("GIT_CONFIG_VALUE_2", "ssh://"),
    (
        "GIT_CONFIG_KEY_3",
        "url.aura-blocked-push://use-git_ship/.pushInsteadOf",
    ),
    ("GIT_CONFIG_VALUE_3", "git@"),
];

/// Refuse a command that pushes: `program` with `args`, or `script` run
/// by a shell.
///
/// # Errors
/// [`ToolError::Forbidden`] with [`RAW_PUSH_REFUSED`] when it would run
/// `git push` or `gh pr merge`.
pub(super) fn refuse_raw_push(
    program: Option<&str>,
    args: &[String],
    script: Option<&str>,
) -> Result<(), ToolError> {
    let mut words: Vec<String> = Vec::new();
    if let Some(program) = program {
        words.push(program.to_string());
        for arg in args {
            // `sh -c '<script>'` and the like carry a script as one word.
            words.extend(shell_words(arg));
        }
    }
    if let Some(script) = script {
        words.extend(shell_words(script));
    }
    if pushes(&words) {
        return Err(ToolError::Forbidden(RAW_PUSH_REFUSED.to_string()));
    }
    Ok(())
}

/// Whether `words` run `git push` or `gh pr merge` anywhere.
fn pushes(words: &[String]) -> bool {
    words.iter().enumerate().any(|(at, word)| {
        let rest = words.get(at + 1..).unwrap_or_default();
        match program_name(word) {
            "git" => git_subcommand(rest) == Some("push"),
            "gh" => {
                let mut sub = rest
                    .iter()
                    .map(String::as_str)
                    .filter(|w| !w.starts_with('-'));
                match sub.next() {
                    Some("pr") => sub.next() == Some("merge"),
                    Some("api") => api_moves_a_branch(rest, GH_DATA_FLAGS),
                    _ => false,
                }
            }
            "curl" => {
                rest.iter().any(|word| word.contains("api.github.com"))
                    && api_moves_a_branch(rest, CURL_DATA_FLAGS)
            }
            _ => false,
        }
    })
}

/// HTTP methods that change something.
const WRITE_METHODS: &[&str] = &["PUT", "POST", "PATCH", "DELETE"];
/// `gh api` flags that send fields, which makes the call a POST.
const GH_DATA_FLAGS: &[&str] = &["-f", "-F", "--field", "--raw-field", "--input"];
/// `curl` flags that send a body.
const CURL_DATA_FLAGS: &[&str] = &[
    "-d",
    "--data",
    "--data-raw",
    "--data-binary",
    "--data-urlencode",
    "--json",
    "-T",
    "--upload-file",
];
/// GraphQL mutations that merge a PR or move a ref.
const GRAPHQL_MOVES: &[&str] = &[
    "mergePullRequest",
    "enablePullRequestAutoMerge",
    "mergeBranch",
    "updateRef",
    "updateRefs",
    "createRef",
    "deleteRef",
];

/// Whether a GitHub API call whose words are `args` merges a PR or moves
/// a branch: a PR's merge endpoint (any method), a write to `/merges` or
/// `/git/refs`, or a GraphQL mutation that merges or moves a ref.
/// `data_flags` are the client's flags that make a call a write.
fn api_moves_a_branch(args: &[String], data_flags: &[&str]) -> bool {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    if words.iter().any(|word| merges_a_pr(word)) {
        return true;
    }
    if words
        .iter()
        .any(|word| GRAPHQL_MOVES.iter().any(|name| word.contains(name)))
    {
        return true;
    }
    let branch_path = words.iter().any(|word| {
        let path = word.split('?').next().unwrap_or(word).trim_end_matches('/');
        path.ends_with("/merges") || path.contains("/git/refs")
    });
    branch_path && writes(&words, data_flags)
}

/// Whether `words` ask for a write: a write method (`-X`, `--method`,
/// `--request`), or one of `data_flags`.
fn writes(words: &[&str], data_flags: &[&str]) -> bool {
    words.iter().enumerate().any(|(at, word)| {
        let method = match *word {
            "-X" | "--method" | "--request" => words.get(at + 1).copied(),
            _ => word
                .strip_prefix("--method=")
                .or_else(|| word.strip_prefix("--request="))
                .or_else(|| word.strip_prefix("-X").filter(|m| !m.is_empty())),
        };
        let write_method = method.is_some_and(|method| {
            WRITE_METHODS
                .iter()
                .any(|write| method.eq_ignore_ascii_case(write))
        });
        write_method
            || data_flags
                .iter()
                .any(|flag| word == flag || word.starts_with(&format!("{flag}=")))
    })
}

/// Whether an API path merges a PR: `.../pulls/<n>/merge`.
fn merges_a_pr(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path).trim_end_matches('/');
    let mut parts = path.rsplit('/');
    parts.next() == Some("merge")
        && parts
            .next()
            .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
        && parts.next() == Some("pulls")
}

/// The name of the program `word` runs: its last path part, without
/// `.exe`.
fn program_name(word: &str) -> &str {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word);
    name.strip_suffix(".exe").unwrap_or(name)
}

/// The subcommand of a git run whose words after `git` are `rest`: the
/// first word that isn't a global option or an option's value.
fn git_subcommand(rest: &[String]) -> Option<&str> {
    let mut words = rest.iter().map(String::as_str);
    while let Some(word) = words.next() {
        match word {
            // Global options that take the next word as their value.
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path"
            | "--config-env" | "--super-prefix" => {
                words.next();
            }
            _ if word.starts_with('-') => {}
            _ => return Some(word),
        }
    }
    None
}

/// `text` split as a shell would split it into words, near enough to
/// find commands: quotes are dropped (so a quoted script's words count
/// too), and separators (`;`, `&`, `|`, newlines, parentheses,
/// backticks, `$(`) end a word.
fn shell_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        let ends_word = c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '`' | '$');
        if ends_word {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else if !matches!(c, '\'' | '"' | '\\') {
            word.push(c);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(text: &str) -> Result<(), ToolError> {
        refuse_raw_push(None, &[], Some(text))
    }

    fn direct(program: &str, args: &[&str]) -> Result<(), ToolError> {
        let args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
        refuse_raw_push(Some(program), &args, None)
    }

    #[test]
    fn every_way_to_type_a_push_is_refused() {
        for text in [
            "git push",
            "git push origin main",
            "git push origin HEAD:main --force",
            "cd repo && git push -u origin aura/x",
            "git -C /work -c user.name=x push",
            "git --git-dir=.git push",
            "/usr/bin/git push",
            "env GIT_TRACE=1 git push",
            "echo hi; git push",
            "sh -c 'git push origin main'",
            "eval \"git push\"",
            "ls | xargs git push",
            "$(git push)",
            "gh pr merge 12 --merge",
            "gh api -X PUT repos/o/r/pulls/12/merge -f merge_method=merge",
            "gh api --method PUT /repos/o/r/pulls/7/merge",
            "gh api -X PATCH repos/o/r/git/refs/heads/main -f sha=abc",
            "gh api repos/o/r/git/refs -f ref=refs/heads/main -f sha=abc",
            "gh api -X POST repos/o/r/merges -f base=main -f head=x",
            "gh api graphql -f query='mutation { mergePullRequest(input: {pullRequestId: \"x\"}) { clientMutationId } }'",
            "gh api graphql -f query='mutation { enablePullRequestAutoMerge(input: {}) { clientMutationId } }'",
            "curl -X PUT -H \"Authorization: token $GH_TOKEN\" https://api.github.com/repos/o/r/pulls/3/merge",
            "curl -X PATCH -H 'Authorization: Bearer t' https://api.github.com/repos/o/r/git/refs/heads/main -d '{}'",
            "curl -H 'Authorization: Bearer t' https://api.github.com/repos/o/r/merges --data '{}'",
            "curl https://api.github.com/graphql -d '{\"query\":\"mutation { mergePullRequest(input:{}) { clientMutationId } }\"}'",
        ] {
            let refused = script(text).expect_err(text);
            assert!(
                refused.to_string().contains("git_ship"),
                "{text}: {refused}"
            );
        }
        assert!(direct("git", &["push", "origin", "main"]).is_err());
        assert!(direct("git", &["-C", "/w", "push"]).is_err());
        assert!(direct("sh", &["-c", "git push origin aura/x"]).is_err());
        assert!(direct("git.exe", &["push"]).is_err());
    }

    #[test]
    fn other_git_and_gh_commands_run() {
        for text in [
            "git status",
            "git fetch origin main",
            "git log --oneline -5",
            "git pull --rebase",
            "git commit -m 'push the button'",
            "git stash push -m wip",
            "gh pr view 12",
            "gh api repos/o/r/pulls/12",
            "gh api repos/o/r/pulls/12/merge-queue-notes",
            "gh api repos/o/r/git/refs/heads/main",
            "gh api repos/o/r/merges",
            "gh api graphql -f query='query { viewer { login } }'",
            "curl -s https://api.github.com/repos/o/r/git/refs/heads/main",
            "curl -X POST https://example.com/git/refs -d x",
            "cargo test -p push-utils",
            "echo pushing",
        ] {
            assert!(script(text).is_ok(), "{text}");
        }
        assert!(
            direct("git", &["-C", "push", "status"]).is_ok(),
            "`push` is -C's value"
        );
        assert!(
            direct("git", &["commit", "-m", "git push later"]).is_err(),
            "text naming a push is refused too"
        );
    }

    #[test]
    fn the_environment_rewrites_every_network_push_url() {
        let count: usize = PUSH_BLOCK_ENV[0].1.parse().unwrap();
        assert_eq!(PUSH_BLOCK_ENV.len(), 1 + 2 * count);
        for pair in PUSH_BLOCK_ENV[1..].chunks(2) {
            assert_eq!(pair[0].1, format!("url.{BLOCKED_PUSH_URL}.pushInsteadOf"));
        }
        let rewritten: Vec<&str> = PUSH_BLOCK_ENV[1..]
            .chunks(2)
            .map(|pair| pair[1].1)
            .collect();
        assert_eq!(rewritten, ["https://", "http://", "ssh://", "git@"]);
    }
}
