//! The command lines of a turn's `run_command` calls.

use serde_json::Value;

/// Zero-width characters a command may hold by a slip.
const ZERO_WIDTH: [char; 5] = ['\u{200b}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{feff}'];

/// The command line a `run_command` call runs: its script, or its program
/// and arguments, without the stray zero-width characters that
/// `run_command` drops before running them ([`without_stray_zero_width`]).
#[must_use]
pub fn command_line(input: &Value) -> String {
    if let Some(script) = ["shell_script", "command"]
        .iter()
        .find_map(|key| input[*key].as_str())
    {
        return without_stray_zero_width(script.to_string());
    }
    let mut words = vec![input["program"].as_str().unwrap_or_default()];
    if let Some(args) = input["args"].as_array() {
        words.extend(args.iter().filter_map(Value::as_str));
    }
    without_stray_zero_width(words.join(" "))
}

/// `text` without the zero-width characters next to ASCII or at its ends,
/// where they can only be slips (`python\u{200b} -m pytest`). Between
/// other characters they join emoji or shape a script's letters, so they
/// stay. `run_command` drops the same ones, so the two must agree.
fn without_stray_zero_width(text: String) -> String {
    if !text.contains(&ZERO_WIDTH[..]) {
        return text;
    }
    let chars: Vec<char> = text.chars().collect();
    let ascii_or_end =
        |at: Option<usize>| at.and_then(|at| chars.get(at)).is_none_or(char::is_ascii);
    chars
        .iter()
        .enumerate()
        .filter(|&(at, c)| {
            !ZERO_WIDTH.contains(c)
                || !(ascii_or_end(at.checked_sub(1)) || ascii_or_end(Some(at + 1)))
        })
        .map(|(_, c)| *c)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_stray_zero_width_character_is_dropped_and_one_that_joins_is_kept() {
        let input = json!({"program": "sh", "args": [".swe/run", "python\u{200b} -m pytest -q \
            test/units/module_utils/urls/test_prepare_multipart.py 2>&1 | tail -60"]});
        assert_eq!(
            command_line(&input),
            "sh .swe/run python -m pytest -q \
             test/units/module_utils/urls/test_prepare_multipart.py 2>&1 | tail -60"
        );
        assert_eq!(
            command_line(&json!({"shell_script": "\u{feff}cargo test\u{2060}"})),
            "cargo test"
        );
        for kept in ["echo 👩\u{200d}💻", "echo क्\u{200d}ष"] {
            assert_eq!(command_line(&json!({"command": kept})), kept);
        }
    }
}
