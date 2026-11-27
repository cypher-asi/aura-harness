//! The lead's limits: the tools it keeps, the commands it may run, and
//! how much of a read or a search it sees.

use super::{
    allows_command, keeps_tool, refused_command, refused_tool, Cap, READ_LINES, SEARCH_HITS,
};

#[test]
fn the_lead_keeps_every_tool_but_the_file_changing_ones() {
    for tool in [
        "write_file",
        "edit_file",
        "delete_file",
        "git_commit",
        "git_commit_push",
        "git_ship",
    ] {
        assert!(!keeps_tool(tool), "{tool}");
    }
    for tool in [
        "read_file",
        "search_code",
        "run_command",
        "task",
        "todo_write",
        "create_plan",
    ] {
        assert!(keeps_tool(tool), "{tool}");
    }
}

#[test]
fn checks_and_inspection_commands_are_allowed() {
    for command in [
        "cargo test -p aura-gate",
        "cargo clippy --workspace -- -D warnings",
        "npm test",
        "pytest -x tests/",
        "git status",
        "git diff --stat",
        "git -C crates/foo log --oneline -5",
        "git --no-pager show HEAD",
        "ls -la src",
        "pwd",
        "cd crates/foo && cargo test",
        "cargo test 2>&1 | tail -20",
        "cargo build 2>/dev/null | grep -c warning",
        "cargo test &>/dev/null",
        "cargo test 1>&2 | tail",
        "git diff; git status",
        "cargo fmt --all -- --check",
        "npx eslint src",
        "ruff check .",
    ] {
        assert!(allows_command(command), "{command}");
    }
}

#[test]
fn other_commands_are_refused() {
    for command in [
        "cat src/main.rs",
        "rm -rf target",
        "sed -i s/a/b/ src/lib.rs",
        "echo hi > notes.txt",
        "cargo test > out.log",
        "cargo test >>/dev/null",
        "cargo test 2>&1x",
        "cargo test >&",
        "git commit -am wip",
        "git checkout -- src",
        "git branch -D main",
        "git diff --output=patch.diff",
        "cargo test && rm -rf src",
        "cargo test $(rm -rf src)",
        "cargo test `whoami`",
        "npm install",
        "cargo run",
        "cargo fmt",
        "ruff format .",
        "head -n 500 src/lib.rs",
        "ls | xargs rm",
        "cargo test &",
        "",
        "  ;  ",
    ] {
        assert!(!allows_command(command), "{command}");
    }
}

#[test]
fn reads_and_searches_are_capped_and_other_tools_are_not() {
    assert_eq!(Cap::of("read_file"), Some(Cap::Read));
    assert_eq!(Cap::of("search_code"), Some(Cap::Search));
    assert_eq!(Cap::of("find_files"), Some(Cap::Search));
    for tool in ["list_files", "run_command", "task"] {
        assert_eq!(Cap::of(tool), None, "{tool}");
    }
}

#[test]
fn long_reads_and_searches_are_capped_with_a_note() {
    let file: String = (1..=250).map(|n| format!("line {n}\n")).collect();
    let capped = Cap::Read.apply(&file).unwrap();
    assert_eq!(capped.lines().count(), READ_LINES + 1);
    assert!(
        capped.contains("line 200\n… [lines capped at 200"),
        "{capped}"
    );
    assert!(capped.contains("50 more left out"));
    assert!(capped.contains("explorer subtask"));
    assert_eq!(Cap::Read.apply("short\nfile"), None);

    let hits: String = (1..=60).map(|n| format!("src/a.rs:{n}: x\n")).collect();
    let capped = Cap::Search.apply(&hits).unwrap();
    assert_eq!(capped.lines().count(), SEARCH_HITS + 1);
    assert!(capped.contains("10 more left out"));
    assert_eq!(Cap::Search.apply("one hit"), None);
}

#[test]
fn caps_and_refusals_read_word_for_word() {
    let file: String = (1..=201).map(|n| format!("{n}\n")).collect();
    let lines: Vec<String> = (1..=200).map(|n| n.to_string()).collect();
    assert_eq!(
        Cap::Read.apply(&file).unwrap(),
        format!(
            "{}\n… [lines capped at 200 for the lead; 1 more left out. Read a narrower \
             start_line/end_line range, or delegate heavy reading to an explorer subtask.]",
            lines.join("\n")
        )
    );
    assert_eq!(Cap::Read.apply(&"x\n".repeat(READ_LINES)), None, "it fits");
    assert_eq!(
        Cap::Search.apply(&"a\r\n".repeat(52)).unwrap(),
        format!(
            "{}\n… [hits capped at 50 for the lead; 2 more left out. Narrow the search, or \
             delegate the search to an explorer subtask.]",
            ["a"; 50].join("\n")
        ),
        "line endings come back as new lines"
    );
    assert_eq!(
        refused_command("rm x"),
        "Not run: under Gate your own run_command only runs checks (tests, builds, linters, \
         type checks) and inspection commands (git status, git diff, git log, git show, ls, \
         pwd). Delegate `rm x` with the task tool: an implement subtask for changes, an \
         explorer subtask for reading."
    );
    assert_eq!(
        refused_tool("write_file"),
        "Not run: under Gate you do not change files yourself. Delegate the edit with the \
         task tool (an implement subtask, with a check command); `write_file` is for \
         subagents."
    );
}
