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
//! nudged ([`crate::followup`]). One run that succeeds is enough: a model
//! that tries its change also tries the inputs it should refuse.

use aura_model::{ContentBlock, Message};

use crate::checks::command_line;

/// Name of the tool that runs commands.
const RUN_COMMAND: &str = "run_command";

/// Tools that change a file.
const FILE_TOOLS: [&str; 3] = ["write_file", "edit_file", "delete_file"];

/// Tools that read one.
const READ_TOOLS: [&str; 1] = ["read_file"];

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

/// Extensions of the files a script imports by name.
const MODULES: &[&str] = &[
    "py", "js", "mjs", "cjs", "ts", "mts", "cts", "jsx", "tsx", "rb", "pl", "pm", "php", "lua",
];

/// Extensions of the other scripts an interpreter runs.
const SCRIPTS: &[&str] = &["sh", "bash", "zsh", "ksh", "awk"];

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
    for message in messages {
        for block in &message.content {
            match block {
                ContentBlock::ToolUse { id, name, input } => {
                    let path = input["path"].as_str().unwrap_or_default();
                    if FILE_TOOLS.contains(&name.as_str()) && !path.is_empty() {
                        changed.push(path);
                        runs.clear();
                    } else if READ_TOOLS.contains(&name.as_str()) && !path.is_empty() {
                        read.push(path);
                    } else if name == RUN_COMMAND && input["background"].as_bool() != Some(true) {
                        let command = command_line(input);
                        if rewrites(&command, &changed) {
                            runs.clear();
                        } else {
                            runs.push((id, command));
                        }
                    }
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error: false,
                    ..
                } => succeeded.push(tool_use_id),
                _ => {}
            }
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
    let words: Vec<&str> = command
        .split_whitespace()
        .map(|word| word.trim_matches(['\'', '"']))
        .collect();
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
    let words: Vec<&str> = command
        .split_whitespace()
        .map(|word| word.trim_matches(['\'', '"']))
        .collect();
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

/// Whether a simple command of `command` starts an interpreter, past
/// variable assignments and wrappers (`env`, `timeout 60`, `uv run`), or
/// runs a script by its path (`./backup.sh`). In `sh -c …` it is the
/// commands after `-c` that run.
fn starts_interpreter(command: &str) -> bool {
    command.split(['&', ';', '|', '\n', '(']).any(|part| {
        let mut words = part
            .split_whitespace()
            .map(|word| word.trim_matches(['\'', '"']));
        while let Some(word) = words.next() {
            if (word.contains('=') && !word.starts_with('-')) || WRAPPERS.contains(&word) {
                continue;
            }
            if matches!(word, "timeout" | "uv" | "poetry" | "pipenv" | "pdm") {
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

/// Whether the file at `path` is a script an interpreter runs.
fn is_script(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, extension)| MODULES.contains(&extension) || SCRIPTS.contains(&extension))
}

/// Whether `command` names the file at `path`: its name, which has an
/// extension and so is not an ordinary word, on its own or ending a path
/// (`backup.sh`, `"$OLDPWD/backup.sh"`, `'store.py'`).
fn names(command: &str, path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    if !name.trim_start_matches('.').contains('.') {
        return false;
    }
    command.match_indices(name).any(|(at, _)| {
        let before = command[..at].chars().next_back();
        let after = command[at + name.len()..].chars().next();
        !before.is_some_and(is_name_char) && !after.is_some_and(is_name_char)
    })
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// The name a script imports the file at `path` by: its name without the
/// extension, or its folder's for a package's `__init__.py` or a
/// folder's `index.js`. `None` for a file not imported by name.
fn module(path: &str) -> Option<&str> {
    let mut parts = path.rsplit('/');
    let (stem, extension) = parts.next()?.rsplit_once('.')?;
    if !MODULES.contains(&extension) {
        return None;
    }
    let stem = if matches!(stem, "__init__" | "index") {
        parts.next()?
    } else {
        stem
    };
    (!stem.is_empty()).then_some(stem)
}

/// Whether code in `command` imports `module`, or `command` runs it:
/// `import shifts`, `import pricing,api`, `from shifts import
/// shift_hours`, `from .shifts import …`, `python -m pkg.shifts`,
/// `require('./money')`, `import x from './money.js'` or
/// `require_relative 'money'`.
fn imports(command: &str, module: &str) -> bool {
    // A path names the module by its last part (`./money.js`), a dotted
    // name by any part (`pkg.money`).
    let is_module = |word: &str| match word.rsplit_once('/') {
        Some((_, name)) => {
            name.rsplit_once('.')
                .filter(|(_, extension)| MODULES.contains(extension))
                .map_or(name, |(stem, _)| stem)
                == module
        }
        None => word.split('.').any(|part| part == module),
    };
    command.split(['\n', ';']).any(|statement| {
        let words: Vec<&str> = statement
            .split(|c: char| c.is_whitespace() || matches!(c, ',' | '(' | ')' | '{' | '}' | '='))
            .map(|word| word.trim_matches(['\'', '"', '`']))
            .filter(|word| !word.is_empty())
            .collect();
        words.iter().enumerate().any(|(at, word)| {
            let rest = &words[at + 1..];
            match *word {
                "from" | "-m" | "require" | "require_relative" => {
                    rest.first().is_some_and(|next| is_module(next))
                }
                "import" => rest
                    .iter()
                    .take_while(|next| **next != "from")
                    .enumerate()
                    .any(|(n, next)| (n == 0 || rest[n - 1] != "as") && is_module(next)),
                _ => false,
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_model::{Role, ToolResultContent};
    use serde_json::{json, Value};

    fn call(id: &str, name: &str, input: Value) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input,
            }],
        )
    }

    fn result(id: &str, is_error: bool) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: ToolResultContent::Text(String::new()),
                is_error,
            }],
        )
    }

    /// An attempt that changed `path` and then ran `run`, which
    /// succeeded unless `failed`.
    fn changed_then_ran(path: &str, run: Value, failed: bool) -> Vec<Message> {
        vec![
            call("1", "write_file", json!({"path": path})),
            result("1", false),
            call("2", RUN_COMMAND, run),
            result("2", failed),
        ]
    }

    #[test]
    fn a_script_that_imports_the_changed_module_checks_the_change() {
        for (path, run) in [
            (
                "store.py",
                json!({"program": "python3", "args": ["-c", "from store import page\nitems=[1, 2]\nprint(page(items, None))"]}),
            ),
            (
                "src/shifts.py",
                json!({"command": "cd src && python3 -c 'import shifts as s; print(s.shift_hours())'"}),
            ),
            (
                "pkg/money/__init__.py",
                json!({"command": "uv run python - <<'EOF'\nfrom pkg.money import parse\nassert parse('1.10') == 110\nEOF"}),
            ),
            (
                "lib/money.js",
                json!({"command": "node -e \"const m = require('./lib/money.js'); console.log(m.round(1.005))\""}),
            ),
            (
                "lib/ansible/modules/mount_facts.py",
                json!({"command": "cd lib && echo '{}' | python -m ansible.modules.mount_facts"}),
            ),
        ] {
            assert!(
                ran(&changed_then_ran(path, run.clone(), false)),
                "{path}: {run}"
            );
        }
    }

    #[test]
    fn running_or_compiling_the_changed_file_checks_it() {
        for (path, run) in [
            (
                "shifts.py",
                json!({"command": "python3 -m py_compile shifts.py"}),
            ),
            (
                "backup.sh",
                json!({"program": "sh", "args": ["-n", "backup.sh"]}),
            ),
            (
                "backup.sh",
                json!({"shell_script": "t=$(mktemp -d); : > \"$t/a b.txt\"; sh backup.sh \"$t\" \"$t/out dir\"; echo rc=$?"}),
            ),
            (
                "scripts/report.py",
                json!({"command": "PYTHONPATH=. python3.11 scripts/report.py --dry-run"}),
            ),
            ("bin/run.sh", json!({"command": "./bin/run.sh check"})),
            (
                "sum.sh",
                json!({"program": "sh", "args": ["-c", "sh sum.sh 10; sh sum.sh 0"]}),
            ),
            (
                "sum.sh",
                json!({"command": "for n in 1 2 100; do sh sum.sh $n; done"}),
            ),
            (
                "store.py",
                json!({"shell_script": "cat > /tmp/t.py <<'EOF'\nfrom store import page\nprint(page([], None))\nEOF\npython3 /tmp/t.py"}),
            ),
            (
                "src/report.py",
                json!({"command": "python3 -m compileall -q src"}),
            ),
            (
                "report.awk",
                json!({"command": "awk -f report.awk data.csv"}),
            ),
        ] {
            assert!(
                ran(&changed_then_ran(path, run.clone(), false)),
                "{path}: {run}"
            );
        }
    }

    #[test]
    fn a_command_that_rewrites_a_changed_file_is_a_change_not_a_check() {
        for edit in [
            json!({"command": "sed -i 's/limit/size/' store.py"}),
            json!({"command": "python3 -c \"s=open('store.py').read().replace('a','b');open('store.py','w').write(s)\""}),
            json!({"command": "printf 'x = 1\\n' >> store.py"}),
            json!({"command": "echo 'x = 1' | tee -a store.py"}),
        ] {
            let mut messages = changed_then_ran(
                "store.py",
                json!({"command": "python3 -m py_compile store.py"}),
                false,
            );
            messages.push(call("3", RUN_COMMAND, edit.clone()));
            messages.push(result("3", false));
            assert!(!ran(&messages), "{edit}");
        }
        let edited_by_script = json!({"shell_script": "f=src/theme.rs\npython3 - \"$f\" <<'EOF'\nimport sys\np=sys.argv[1]; s=open(p).read()\nopen(p,'w').write(s.replace('a','b'))\nEOF"});
        assert!(!ran(&changed_then_ran(
            "src/theme.rs",
            edited_by_script,
            false
        )));
    }

    #[test]
    fn running_code_the_attempt_read_checks_what_it_uses() {
        for run in [
            json!({"program": "python", "args": ["-c", "import api;print(api.price_view('widget','EUR'))"]}),
            json!({"command": "python api.py --help"}),
        ] {
            let messages = [
                call("0", "read_file", json!({"path": "api.py"})),
                result("0", false),
                call("1", "edit_file", json!({"path": "cache.py"})),
                result("1", false),
                call("2", RUN_COMMAND, run.clone()),
                result("2", false),
            ];
            assert!(ran(&messages), "{run}");
        }
    }

    #[test]
    fn a_run_that_failed_or_came_before_the_last_change_does_not() {
        let import = json!({"command": "python3 -c 'from store import page'"});
        assert!(!ran(&changed_then_ran("store.py", import.clone(), true)));

        let mut refused = changed_then_ran("store.py", import.clone(), false);
        refused.push(call(
            "3",
            RUN_COMMAND,
            json!({"command": "python3 store.py --size -1"}),
        ));
        refused.push(result("3", true));
        assert!(
            ran(&refused),
            "an input it should refuse, tried after one that ran"
        );

        let mut changed_again = changed_then_ran("store.py", import.clone(), false);
        changed_again.push(call("3", "edit_file", json!({"path": "store.py"})));
        changed_again.push(result("3", false));
        assert!(!ran(&changed_again), "the code changed after it ran");

        let mut passed_last = changed_then_ran("store.py", import.clone(), true);
        passed_last.push(call("3", RUN_COMMAND, import.clone()));
        passed_last.push(result("3", false));
        passed_last.push(call("4", RUN_COMMAND, json!({"command": "ls"})));
        passed_last.push(result("4", true));
        assert!(
            ran(&passed_last),
            "a failed run of something else changes nothing"
        );

        assert!(
            !ran(&[call("2", RUN_COMMAND, import), result("2", false)]),
            "nothing changed"
        );
    }

    #[test]
    fn a_run_that_does_not_run_the_changed_code_does_not() {
        for run in [
            json!({"command": "cat store.py"}),
            json!({"command": "rg 'from store import' ."}),
            json!({"command": "python3 -c 'import json; print(json.dumps([1]))'"}),
            json!({"command": "python3 -c 'from storage import open_store'"}),
            json!({"command": "python3 -m py_compile other.py"}),
            json!({"command": "python3 -c 'from store import page'", "background": true}),
            json!({"program": "sh", "args": ["-c", "git diff --stat -- store.py"]}),
        ] {
            assert!(
                !ran(&changed_then_ran("store.py", run.clone(), false)),
                "{run}"
            );
        }
        let notes = json!({"command": "python3 -c \"print(open('NOTES.md').read())\""});
        assert!(
            !ran(&changed_then_ran("NOTES.md", notes, false)),
            "not a script"
        );
    }

    #[test]
    fn a_file_is_named_by_its_whole_name() {
        assert!(names("sh \"$OLDPWD/backup.sh\" src", "backup.sh"));
        assert!(names(
            "py_compile.compile('store.py',doraise=True)",
            "store.py"
        ));
        assert!(!names("cp backup.sh.bak x", "backup.sh"));
        assert!(!names("sh mybackup.sh", "backup.sh"));
        assert!(
            !names("make && ./run check", "run"),
            "a name with no extension"
        );
    }
}
