use super::*;
use crate::file_ops::stub_detection::{StubPattern, StubReport};
use crate::verify::error_types::{parse_error_references, BuildFixAttemptRecord};

fn test_project() -> ProjectInfo<'static> {
    ProjectInfo {
        name: "test",
        description: "Test project",
        folder_path: "/tmp/test",
        build_command: Some("cargo build"),
        test_command: Some("cargo test"),
    }
}

fn test_spec(content: &str) -> SpecInfo<'_> {
    SpecInfo {
        title: "Test Spec",
        markdown_contents: content,
    }
}

fn test_task<'a>(title: &'a str, desc: &'a str) -> TaskInfo<'a> {
    TaskInfo {
        title,
        description: desc,
        execution_notes: "",
        files_changed: &[],
    }
}

fn test_session() -> SessionInfo<'static> {
    SessionInfo {
        summary_of_previous_context: "",
    }
}

#[allow(clippy::too_many_arguments)] // TODO(W3): wrap inputs in `BuildFixPromptParams`.
fn build_fix_prompt(
    project: &ProjectInfo<'_>,
    spec: &SpecInfo<'_>,
    task: &TaskInfo<'_>,
    session: &SessionInfo<'_>,
    codebase_snapshot: &str,
    build_command: &str,
    stderr: &str,
    stdout: &str,
    prior_notes: &str,
) -> String {
    build_fix_prompt_with_history(&BuildFixPromptParams {
        project,
        spec,
        task,
        session,
        codebase_snapshot,
        build_command,
        stderr,
        stdout,
        prior_notes,
        prior_attempts: &[],
    })
}

#[test]
fn test_build_fix_prompt_contains_error_output() {
    let project = test_project();
    let spec = test_spec("spec content");
    let task = test_task("Fix build", "Fix the build errors");
    let session = test_session();
    let prompt = build_fix_prompt(
        &project,
        &spec,
        &task,
        &session,
        "",
        "cargo build",
        "error[E0308]: mismatched types",
        "Compiling test v0.1.0",
        "initial notes",
    );
    assert!(
        prompt.contains("error[E0308]"),
        "stderr should be in prompt"
    );
    assert!(
        prompt.contains("Compiling test"),
        "stdout should be in prompt"
    );
}

#[test]
fn test_build_fix_prompt_contains_task_and_spec() {
    let project = test_project();
    let spec = test_spec("implement login flow");
    let task = test_task("Add login handler", "Create the login endpoint");
    let session = test_session();
    let prompt = build_fix_prompt(
        &project,
        &spec,
        &task,
        &session,
        "",
        "cargo build",
        "error: cannot find function",
        "",
        "",
    );
    assert!(
        prompt.contains("Add login handler"),
        "task title should be in prompt"
    );
    assert!(
        prompt.contains("implement login flow"),
        "spec content should be in prompt"
    );
}

#[test]
fn test_build_fix_prompt_with_history_includes_prior_attempts() {
    let project = test_project();
    let spec = test_spec("spec");
    let task = test_task("Fix it", "Fix");
    let session = test_session();
    let prior = vec![BuildFixAttemptRecord {
        stderr: "first error".into(),
        error_signature: "sig1".into(),
        files_changed: vec!["src/main.rs".into()],
        changes_summary: "changed main".into(),
    }];
    let params = BuildFixPromptParams {
        project: &project,
        spec: &spec,
        task: &task,
        session: &session,
        codebase_snapshot: "",
        build_command: "cargo build",
        stderr: "second error",
        stdout: "",
        prior_notes: "",
        prior_attempts: &prior,
    };
    let prompt = build_fix_prompt_with_history(&params);
    assert!(
        prompt.contains("Previous Fix Attempts"),
        "should mention prior attempts"
    );
    assert!(
        prompt.contains("first error"),
        "prior error should be included"
    );
    assert!(
        prompt.contains("changed main"),
        "prior changes should be included"
    );
}

#[test]
fn test_build_fix_prompt_with_history_empty_prior() {
    let project = test_project();
    let spec = test_spec("spec");
    let task = test_task("Fix", "Fix");
    let session = test_session();
    let params = BuildFixPromptParams {
        project: &project,
        spec: &spec,
        task: &task,
        session: &session,
        codebase_snapshot: "",
        build_command: "cargo build",
        stderr: "some error",
        stdout: "",
        prior_notes: "",
        prior_attempts: &[],
    };
    let prompt = build_fix_prompt_with_history(&params);
    assert!(
        !prompt.contains("Previous Fix Attempts"),
        "no prior section when empty"
    );
}

#[test]
fn test_detect_api_hallucination_flags_method_not_found() {
    let mut categories = vec![];
    let refs = ErrorReferences {
        types_referenced: vec![],
        methods_not_found: vec![
            ("MyStruct".into(), "method_a".into()),
            ("MyStruct".into(), "method_b".into()),
            ("MyStruct".into(), "method_c".into()),
        ],
        missing_fields: vec![],
        source_locations: vec![],
        wrong_arg_counts: vec![],
    };
    detect_api_hallucination(&refs, &mut categories);
    assert!(categories
        .iter()
        .any(|c| matches!(c, ErrorCategory::RustApiHallucination)));
}

#[test]
fn test_truncate_prompt_output_within_limit() {
    let short = "hello world";
    let result = truncate_prompt_output(short, 1000);
    assert_eq!(result, short);
}

#[test]
fn test_truncate_prompt_output_over_limit() {
    let long = "x".repeat(10_000);
    let result = truncate_prompt_output(&long, 200);
    assert!(result.len() < long.len());
    assert!(result.contains("truncated"));
}

#[test]
fn test_build_stub_fix_prompt_single_report() {
    let reports = vec![StubReport {
        path: "src/lib.rs".into(),
        line: 42,
        pattern: StubPattern::TodoMacro,
        context: "fn do_thing() { todo!() }".into(),
    }];
    let prompt = build_stub_fix_prompt(&reports);
    assert!(prompt.contains("src/lib.rs:42"));
    assert!(prompt.contains("todo!()"));
    assert!(prompt.contains("stub/placeholder"));
}

#[test]
fn test_build_stub_fix_prompt_multiple_reports() {
    let reports = vec![
        StubReport {
            path: "a.rs".into(),
            line: 1,
            pattern: StubPattern::TodoMacro,
            context: "ctx1".into(),
        },
        StubReport {
            path: "b.rs".into(),
            line: 2,
            pattern: StubPattern::UnimplementedMacro,
            context: "ctx2".into(),
        },
    ];
    let prompt = build_stub_fix_prompt(&reports);
    assert!(prompt.contains("a.rs:1"));
    assert!(prompt.contains("b.rs:2"));
    assert!(prompt.contains("ctx1"));
    assert!(prompt.contains("ctx2"));
}

#[test]
fn parse_error_references_extracts_methods_and_types() {
    let stderr = r#"error[E0599]: no method named `foo` found for struct `MyStruct` in the current scope
  --> src/main.rs:10:5
error[E0599]: no method named `bar` found for struct `MyStruct` in the current scope
  --> src/main.rs:15:5"#;
    let refs = parse_error_references(stderr);
    assert!(refs.types_referenced.contains(&"MyStruct".to_string()));
    assert_eq!(refs.methods_not_found.len(), 2);
    assert_eq!(refs.source_locations.len(), 2);
}

#[test]
fn parse_error_references_extracts_missing_fields() {
    let stderr = r#"error[E0063]: missing field `name` in initializer of `crate::types::User`"#;
    let refs = parse_error_references(stderr);
    assert!(refs
        .missing_fields
        .iter()
        .any(|(t, f)| t == "User" && f == "name"));
}

#[test]
fn parse_error_references_extracts_wrong_arg_counts() {
    let stderr = "this function takes 2 arguments but 3 arguments were supplied";
    let refs = parse_error_references(stderr);
    assert_eq!(refs.wrong_arg_counts.len(), 1);
    assert!(refs.wrong_arg_counts[0].contains("expected 2 got 3"));
}
