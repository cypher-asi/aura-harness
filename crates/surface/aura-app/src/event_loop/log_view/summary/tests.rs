use super::*;
use serde_json::json;

fn output<'a>(tool: &'a str, text: &'a str) -> Output<'a> {
    Output {
        tool,
        text,
        complete: true,
        chars: text.chars().count(),
        is_error: false,
        source: "executor",
        files: &[],
    }
}

#[test]
fn a_call_reads_as_its_tool_and_key_argument() {
    let label = |name, input| call_label(name, &input);
    assert_eq!(
        label(
            "read_file",
            json!({"path": "crates/x.rs", "start_line": 10, "end_line": 40})
        ),
        "read_file crates/x.rs L10-40"
    );
    assert_eq!(
        label("search_code", json!({"pattern": "fn foo", "path": "src"})),
        "search_code \"fn foo\" in src"
    );
    assert_eq!(
        label(
            "run_command",
            json!({"program": "cargo", "args": ["test", "-q"]})
        ),
        "run_command cargo test -q"
    );
    assert_eq!(
        label(
            "run_command",
            json!({"shell_script": "cargo build\ncargo test"})
        ),
        "run_command cargo build"
    );
    assert_eq!(label("list_files", json!({"path": "."})), "list_files .");
    assert_eq!(
        label(
            "task",
            json!({"subagent_type": "explore", "prompt": "find it"})
        ),
        "task @explore find it"
    );
    assert_eq!(
        label("web_fetch", json!({"url": "https://x.dev"})),
        "web_fetch https://x.dev"
    );
    assert_eq!(label("todo", json!({})), "todo");
}

#[test]
fn listings_and_reads_are_counted() {
    let listing = "dir\t4096\t.aura\ndir\t4096\tcrates\nfile\t1294\tmain.rs";
    assert_eq!(result_summary(&output("list_files", listing)), "3 entries");
    let read = "1|<!DOCTYPE html>\n2|<html>\n3|</html>\n... [truncated: showing lines 1-3 of 9]";
    assert_eq!(result_summary(&output("read_file", read)), "3 lines");
    assert_eq!(
        result_summary(&output("find_files", "No files found")),
        "no files"
    );
    let none = "No matches found. Tip: check that special regex characters are escaped";
    assert_eq!(result_summary(&output("search_code", none)), "no matches");
    assert_eq!(
        result_summary(&output("search_code", "a.rs:1:fn foo()")),
        "1 match"
    );
    assert_eq!(
        result_summary(&output(
            "stat_file",
            "is_file=true\nsize=116874\nis_dir=false"
        )),
        "file · 115 KB"
    );
}

#[test]
fn the_start_of_a_longer_output_gives_an_estimate() {
    let start = "1|aaaaaaaa\n2|bbbbbbbb\n3|cccc…";
    let cut = Output {
        complete: false,
        chars: 2_200,
        ..output("read_file", start)
    };
    assert_eq!(result_summary(&cut), "~200 lines");
}

#[test]
fn commands_say_their_exit_and_how_much_they_printed() {
    let envelope = r#"{"exit_code":0,"ok":true,"stderr":"","stdout":"a\nb\nc\n"}"#;
    assert_eq!(
        result_summary(&output("run_command", envelope)),
        "exit 0 · 3 lines"
    );
    let failed = r#"{"exit_code":101,"ok":false,"stderr":"","stdout":""}"#;
    let failed = Output {
        is_error: true,
        ..output("run_command", failed)
    };
    assert_eq!(result_summary(&failed), "exit 101 · no output");
    let start = r#"{"exit_code":0,"ok":true,"stderr":"","stdout":"crates/ui/a.rs\ncrates/ui/b.rs\ncrates/u…"#;
    let cut = Output {
        complete: false,
        chars: 2_000,
        ..output("run_command", start)
    };
    let said = result_summary(&cut);
    assert!(said.starts_with("exit 0 · ~"), "{said}");
    assert!(!said.contains('{'), "{said}");
}

#[test]
fn errors_writes_and_blocks_say_what_happened() {
    let error = Output {
        is_error: true,
        ..output("read_file", "Tool error: no such file\nat crates/x.rs")
    };
    assert_eq!(result_summary(&error), "error: Tool error: no such file");
    let files = [FileTouch {
        path: "a.rs".into(),
        kind: "modify".into(),
        added: 12,
        removed: 3,
    }];
    let edit = Output {
        files: &files,
        ..output("edit_file", "Replaced 1 occurrence(s) in a.rs")
    };
    assert_eq!(result_summary(&edit), "+12 -3");
    let blocked = Output {
        source: "blocked",
        ..output("write_file", "writes are off in plan mode")
    };
    assert_eq!(
        result_summary(&blocked),
        "blocked: writes are off in plan mode"
    );
    let cached = Output {
        source: "cache",
        ..output("find_files", "a.rs\nb.rs")
    };
    assert_eq!(result_summary(&cached), "2 files · cached");
    let reply = json!({"final_message": "Found it.\nMore", "exit": {"kind": "completed"}});
    assert_eq!(
        result_summary(&output("task", &reply.to_string())),
        "Found it."
    );
}
