//! The `--` options of a new comparison, and the values each takes.

use super::{CompareRequest, SessionPick, USAGE};

/// Most runs per arm one comparison may ask for.
const MAX_REPEATS: u32 = 10;

/// What `--learn=` takes for on.
const ON: [&str; 4] = ["on", "yes", "true", "1"];

/// What `--learn=` takes for off.
const OFF: [&str; 4] = ["off", "no", "false", "0"];

/// Set option `--name` of `request` to `value`.
pub(super) fn set_flag(
    request: &mut CompareRequest,
    name: &str,
    value: &str,
) -> Result<(), String> {
    let given = || required(name, value).map(str::to_string);
    match name {
        "plan" => request.plan = Some(given()?),
        "session" => request.session = Some(session(required(name, value)?)?),
        "examples" | "tasks" => request.examples = Some(examples(value)?),
        "suite" => request.suite = Some(given()?),
        "a" => request.a = Some(given()?),
        "against" | "vs" | "b" => request.against = Some(given()?),
        "check" => request.check = Some(given()?),
        "learn" => request.learn = learn(value)?,
        "repeats" | "runs" | "n" => request.repeats = repeats(value)?,
        "budget" => request.budget_usd = Some(budget(value)?),
        other => return Err(format!("Unknown option --{other}. {USAGE}")),
    }
    Ok(())
}

/// `value` trimmed, or the error that `--name` needs a value when nothing
/// is left of it.
fn required<'a>(name: &str, value: &'a str) -> Result<&'a str, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("--{name} needs a value. {USAGE}"));
    }
    Ok(value)
}

/// The session `--session` names: `current`, or a start time.
fn session(value: &str) -> Result<SessionPick, String> {
    if value.eq_ignore_ascii_case("current") {
        return Ok(SessionPick::Current);
    }
    value
        .parse()
        .map(SessionPick::Started)
        .map_err(|_| format!("--session takes `current` or a session's start time, not `{value}`"))
}

/// How many example tasks `--examples` asks for.
fn examples(value: &str) -> Result<usize, String> {
    let tasks = value.trim().parse::<usize>().ok().filter(|n| *n > 0);
    tasks.ok_or_else(|| format!("--examples takes how many example tasks to run, not `{value}`"))
}

/// Whether `--learn=` turns learning on.
fn learn(value: &str) -> Result<bool, String> {
    let word = value.trim();
    let any_of = |words: [&str; 4]| words.iter().any(|w| w.eq_ignore_ascii_case(word));
    if any_of(ON) {
        return Ok(true);
    }
    if any_of(OFF) {
        return Ok(false);
    }
    Err(format!("--learn takes on or off, not `{value}`"))
}

/// Runs per arm `--repeats` asks for.
fn repeats(value: &str) -> Result<u32, String> {
    let repeats = value.trim().parse::<u32>().ok();
    repeats
        .filter(|n| (1..=MAX_REPEATS).contains(n))
        .ok_or_else(|| format!("--repeats takes a number from 1 to {MAX_REPEATS}, not `{value}`"))
}

/// The spend `--budget` stops new runs at, in USD.
fn budget(value: &str) -> Result<f64, String> {
    let budget = value.trim().trim_start_matches('$').parse::<f64>().ok();
    budget
        .filter(|b| b.is_finite() && *b > 0.0)
        .ok_or_else(|| format!("--budget takes an amount in USD such as 2.50, not `{value}`"))
}
