//! The blind judge's reply with the attempts it saw named as the runs
//! they were.

/// What every name of an attempt the judge is asked for holds, after
/// its first letter.
const ATTEMPT: &str = "ttempt ";

/// How long a name of an attempt is: `Attempt A`.
const NAMED: usize = "Attempt A".len();

/// The judge's `reply` with the attempts it saw named as the runs they
/// were, `names` being the arm A run's and the arm B run's: "Attempt A"
/// (or "attempt A") is the arm A run when it was shown first, else the
/// arm B run, and so on. The order is random, so its own labels say
/// nothing about the arms.
#[must_use]
pub fn unblind(reply: &str, a_first: bool, names: (&str, &str)) -> String {
    let (shown_a, shown_b) = if a_first { names } else { (names.1, names.0) };
    let mut out = String::with_capacity(reply.len());
    let mut copied = 0;
    for (at, _) in reply.match_indices(ATTEMPT) {
        let Some(start) = at.checked_sub(1) else {
            continue;
        };
        let name = match label_at(reply, start) {
            Some('A') => shown_a,
            Some('B') => shown_b,
            _ => continue,
        };
        out.push_str(&reply[copied..start]);
        out.push_str(name);
        copied = start + NAMED;
    }
    out.push_str(&reply[copied..]);
    out
}

/// The label of the attempt `reply` names at `start`, as "Attempt A" or
/// "attempt A" with neither a letter nor a digit on either side.
fn label_at(reply: &str, start: usize) -> Option<char> {
    let word = reply.get(start..)?;
    let after = word
        .strip_prefix("Attempt ")
        .or_else(|| word.strip_prefix("attempt "))?;
    let mut chars = after.chars();
    let label = chars.next()?;
    let ends = chars.next().is_none_or(|c| !c.is_alphanumeric());
    let before = reply[..start].chars().next_back();
    let starts = before.is_none_or(|c| !c.is_alphanumeric());
    (starts && ends).then_some(label)
}
