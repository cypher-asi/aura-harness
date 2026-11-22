//! Whether an attempt checked its change by running it itself.
//!
//! Where no test covers a change, as in a project with no tests at all,
//! what a model can run is the changed code: a script that imports the
//! module it changed, or one it read that uses it, and prints or asserts
//! what it returns; the changed script run on a few inputs; or a syntax
//! check of the changed file (`python -m py_compile`, `sh -n`). No test
//! runner or build reports on such a run, so it is not a check
//! ([`crate::checks`]) and does not say whether the work is right. It is
//! what Gate's nudge asks for when no test covers a change, though, so
//! work that ran one after its last change, and saw it succeed, is not
//! nudged (`aura-gate-followup`). One run that succeeds is enough: a model
//! that tries its change also tries the inputs it should refuse.
//!
//! - `interpreter`: whether a command starts an interpreter
//! - `names`: whether a command names a file, or imports it as a module

use aura_model::Message;

use crate::transcript::{tools, Tool, FILE_TOOLS};

mod interpreter;
mod names;
#[cfg(test)]
mod tests;

use interpreter::starts_interpreter;
use names::{imports, is_script, module, names};

/// Tools that read a file.
const READ_TOOLS: [&str; 1] = ["read_file"];

/// What a command that rewrites a file in place says.
const IN_PLACE: &[&str] = &[
    ".write(",
    "write_text(",
    "writelines(",
    "writeFile",
    "sed -i",
    "perl -i",
    "perl -pi",
];

/// Whether `messages`, an attempt's, show it running the code it changed
/// with its file tools, after the last such change, and such a run
/// succeeding: a command that starts an interpreter and names a script
/// the attempt changed or read, hands it code that imports a module the
/// attempt changed or read, or byte-compiles a folder with a changed
/// file in it. A run in the background says nothing, and a command that
/// rewrites a changed file is another change.
#[must_use]
pub fn ran(messages: &[Message]) -> bool {
    let mut changed: Vec<&str> = Vec::new();
    let mut read: Vec<&str> = Vec::new();
    // The commands run since the last change, with their call ids.
    let mut runs: Vec<(&str, String)> = Vec::new();
    let mut succeeded: Vec<&str> = Vec::new();
    for tool in tools(messages) {
        match tool {
            Tool::Call { name, input, .. } => {
                let path = input["path"].as_str().unwrap_or_default();
                if path.is_empty() {
                    continue;
                }
                if FILE_TOOLS.contains(&name) {
                    changed.push(path);
                    runs.clear();
                } else if READ_TOOLS.contains(&name) {
                    read.push(path);
                }
            }
            Tool::Command {
                id,
                line,
                background: false,
                ..
            } => {
                if rewrites(&line, &changed) {
                    runs.clear();
                } else {
                    runs.push((id, line));
                }
            }
            Tool::Result {
                id,
                is_error: false,
                ..
            } => succeeded.push(id),
            Tool::Command { .. } | Tool::Result { .. } => {}
        }
    }
    runs.iter()
        .any(|(id, command)| succeeded.contains(id) && runs_changed_code(command, &changed, &read))
}

/// Whether `command` runs the code the attempt `changed`: it starts an
/// interpreter and names a script the attempt changed or `read`, which
/// may use what changed, imports a module of either, or byte-compiles a
/// folder that holds a changed file.
fn runs_changed_code(command: &str, changed: &[&str], read: &[&str]) -> bool {
    let seen = || changed.iter().chain(read);
    starts_interpreter(command)
        && (seen().any(|path| is_script(path) && names(command, path))
            || seen()
                .filter_map(|path| module(path))
                .any(|module| imports(command, module))
            || compiles(command, changed))
}

/// Whether `command` byte-compiles a folder that holds a changed Python
/// file (`python -m compileall -q src`).
fn compiles(command: &str, changed: &[&str]) -> bool {
    let words = unquoted_words(command);
    let Some(at) = words.iter().position(|word| *word == "compileall") else {
        return false;
    };
    let folders: Vec<&str> = words[at + 1..]
        .iter()
        .take_while(|word| !matches!(**word, "&&" | "||" | ";" | "|"))
        .filter(|word| !word.starts_with('-'))
        .map(|folder| folder.trim_start_matches("./").trim_end_matches('/'))
        .collect();
    changed
        .iter()
        .filter(|path| path.ends_with(".py"))
        .any(|path| {
            let path = path.trim_start_matches("./");
            folders.iter().any(|folder| {
                matches!(*folder, "" | ".")
                    || path.starts_with(&format!("{folder}/"))
                    || path.contains(&format!("/{folder}/"))
            })
        })
}

/// Whether `command` writes over a file the attempt `changed`: it names
/// the file and rewrites a file in place (`sed -i`, `open(…, 'w').write(…)`)
/// or sends output to it (`> store.py`, `tee -a store.py`).
fn rewrites(command: &str, changed: &[&str]) -> bool {
    let words = unquoted_words(command);
    changed.iter().any(|path| {
        names(command, path)
            && (IN_PLACE.iter().any(|write| command.contains(write))
                || words.iter().enumerate().any(|(at, word)| {
                    let before = |back: usize| at.checked_sub(back).map(|at| words[at]);
                    names(word, path)
                        && (word.starts_with('>')
                            || before(1).is_some_and(|word| word.ends_with('>') || word == "tee")
                            || (before(1) == Some("-a") && before(2) == Some("tee")))
                }))
    })
}

/// The words of `command`, without the quotes around them.
fn unquoted_words(command: &str) -> Vec<&str> {
    command
        .split_whitespace()
        .map(|word| word.trim_matches(['\'', '"']))
        .collect()
}
