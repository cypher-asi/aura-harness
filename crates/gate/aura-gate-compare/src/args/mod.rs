//! Reading the arguments of `/compare`: a subcommand, or what a new
//! comparison runs.

mod flags;
mod subcommand;
mod words;

#[cfg(test)]
mod tests;

use flags::set_flag;
use subcommand::subcommand;
use words::{words, Word};

/// Runs per arm when the command names no number.
const DEFAULT_REPEATS: u32 = 3;

/// How `/compare` is used, for error messages.
const USAGE: &str = "Usage: /compare [prompt | --plan <file> | --session current|<started_ms> | --examples N | --suite <name>] \
[--a <model|gate|difficulty:easy=<model>,medium=<model>,hard=<model>>] \
[--against <model|gate|gate:nodelegate|difficulty:easy=<model>,medium=<model>,hard=<model>>] \
[--repeats N] [--check <cmd>] [--budget $] [--learn] · \
/compare go | cancel | show [n] | diff [run] | apply <run> | dismiss";

/// Which earlier session a comparison replays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPick {
    /// The session the agent is in.
    Current,
    /// The session that started at this Unix millisecond.
    Started(u64),
}

/// What a comparison runs.
#[derive(Debug, Clone, PartialEq)]
pub struct CompareRequest {
    /// The prompt; `None` uses the user's last prompt (or `plan`, or
    /// `session`).
    pub prompt: Option<String>,
    /// A plan file to `/build` in each copy instead of a prompt.
    pub plan: Option<String>,
    /// A session whose prompts each run replays instead of a prompt.
    pub session: Option<SessionPick>,
    /// What arm A runs on; `None` for Gate.
    pub a: Option<String>,
    /// What arm B runs on; `None` for the strongest Opus.
    pub against: Option<String>,
    /// Runs per arm.
    pub repeats: u32,
    /// Command run in each copy afterwards; `None` for the project's.
    pub check: Option<String>,
    /// Spend after which no new run starts, in USD.
    pub budget_usd: Option<f64>,
    /// How many of the built-in example coding tasks to run instead of a
    /// prompt; `repeats` runs of each per arm.
    pub examples: Option<usize>,
    /// Whether Gate arms explore as routed turns do, so the runs teach
    /// Gate about models it would not pick yet.
    pub learn: bool,
    /// The suite the example tasks are taken from; `None` for every
    /// suite. With no `examples`, every task of the suite.
    pub suite: Option<String>,
}

impl Default for CompareRequest {
    fn default() -> Self {
        Self {
            prompt: None,
            plan: None,
            session: None,
            a: None,
            against: None,
            repeats: DEFAULT_REPEATS,
            check: None,
            budget_usd: None,
            examples: None,
            learn: false,
            suite: None,
        }
    }
}

/// What `/compare <args>` asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum CompareCommand {
    /// `/compare` or `/compare list`: past comparisons.
    List,
    /// `/compare show [n]`: comparison `n` (1-based), or the latest.
    Show(Option<usize>),
    /// Prepare a comparison and show its estimate.
    Start(CompareRequest),
    /// `/compare go`: start the comparison waiting for confirmation.
    Go,
    /// `/compare cancel`: drop the one waiting, or stop the one running.
    Cancel,
    /// `/compare apply <run>`: put that run's changes in the workspace.
    Apply(String),
    /// `/compare diff [run]`: show a run's changes, or the first of each
    /// arm.
    Diff(Option<String>),
    /// `/compare dismiss`: remove the copies.
    Dismiss,
}

/// Read the arguments of `/compare`.
///
/// Subcommands (`go`, `apply a-1`, ...) are recognised only when the
/// arguments are exactly that, so a prompt that happens to start with
/// one of their names is still a prompt; `--` makes the rest a prompt
/// whatever it says. The prompt keeps its text as typed.
///
/// # Errors
/// Returns what is wrong with the arguments, with the usage.
pub fn parse(args: &str) -> Result<CompareCommand, String> {
    let words = words(args)?;
    if let Some(command) = subcommand(&words) {
        return command;
    }
    let mut request = CompareRequest::default();
    let prompt = read_options(args, &words, &mut request)?.join(" ");
    let prompt = prompt.trim();
    only_one_source(&request, !prompt.is_empty())?;
    if !prompt.is_empty() {
        request.prompt = Some(prompt.to_string());
    }
    Ok(CompareCommand::Start(request))
}

/// Set `request`'s options from `words`, the words of `args`, and return
/// the other words as typed: the prompt's.
fn read_options<'a>(
    args: &'a str,
    words: &[Word],
    request: &mut CompareRequest,
) -> Result<Vec<&'a str>, String> {
    let mut prompt = Vec::new();
    let mut words = words.iter();
    while let Some(word) = words.next() {
        let raw = word.raw(args);
        if raw == "--" {
            prompt.extend(words.next().map(|next| args[next.start..].trim()));
            break;
        }
        let Some(flag) = raw.strip_prefix("--") else {
            prompt.push(raw);
            continue;
        };
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) => (name, Some(value.trim_matches(['"', '\'']))),
            None => (flag, None),
        };
        if name == "learn" && inline.is_none() {
            request.learn = true;
            continue;
        }
        let value = match inline {
            Some(value) => value,
            None => words
                .next()
                .map(|next| next.text.as_str())
                .ok_or_else(|| format!("--{name} needs a value. {USAGE}"))?,
        };
        set_flag(request, name, value)?;
    }
    Ok(prompt)
}

/// Fail when more than one of a prompt, a plan, a session and example
/// tasks says what the comparison runs.
fn only_one_source(request: &CompareRequest, prompt: bool) -> Result<(), String> {
    let sources = [
        prompt,
        request.plan.is_some(),
        request.session.is_some(),
        request.examples.is_some() || request.suite.is_some(),
    ];
    if sources.into_iter().filter(|given| *given).count() > 1 {
        return Err(format!(
            "Give a prompt, --plan, --session or --examples, not more than one. {USAGE}"
        ));
    }
    Ok(())
}
