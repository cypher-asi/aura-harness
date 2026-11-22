//! The runs that check an attempt's change itself, and those that do not.

use aura_model::Message;
use serde_json::{json, Value};

use super::*;
use crate::test_support::{call, result};
use crate::transcript::RUN_COMMAND;

/// An attempt that changed `path` and then ran `run`, which succeeded
/// unless `failed`.
fn changed_then_ran(path: &str, run: Value, failed: bool) -> Vec<Message> {
    vec![
        call("1", "write_file", json!({"path": path})),
        result("1", "", false),
        call("2", RUN_COMMAND, run),
        result("2", "", failed),
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
        messages.push(result("3", "", false));
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
            result("0", "", false),
            call("1", "edit_file", json!({"path": "cache.py"})),
            result("1", "", false),
            call("2", RUN_COMMAND, run.clone()),
            result("2", "", false),
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
    refused.push(result("3", "", true));
    assert!(
        ran(&refused),
        "an input it should refuse, tried after one that ran"
    );

    let mut changed_again = changed_then_ran("store.py", import.clone(), false);
    changed_again.push(call("3", "edit_file", json!({"path": "store.py"})));
    changed_again.push(result("3", "", false));
    assert!(!ran(&changed_again), "the code changed after it ran");

    let mut passed_last = changed_then_ran("store.py", import.clone(), true);
    passed_last.push(call("3", RUN_COMMAND, import.clone()));
    passed_last.push(result("3", "", false));
    passed_last.push(call("4", RUN_COMMAND, json!({"command": "ls"})));
    passed_last.push(result("4", "", true));
    assert!(
        ran(&passed_last),
        "a failed run of something else changes nothing"
    );

    assert!(
        !ran(&[call("2", RUN_COMMAND, import), result("2", "", false)]),
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
