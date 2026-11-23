use super::*;
use crate::NO_DELEGATE;

fn start(args: &str) -> CompareRequest {
    match parse(args) {
        Ok(CompareCommand::Start(request)) => request,
        other => panic!("{args:?} gave {other:?}"),
    }
}

/// A new comparison of `prompt`, with every option at its default.
fn asked(prompt: &str) -> CompareRequest {
    CompareRequest {
        prompt: Some(prompt.to_string()),
        ..CompareRequest::default()
    }
}

#[test]
fn no_arguments_list_past_comparisons() {
    assert_eq!(parse(""), Ok(CompareCommand::List));
    assert_eq!(parse("  list "), Ok(CompareCommand::List));
}

#[test]
fn flags_and_the_prompt_are_read_in_any_order() {
    let request = start(
        r#"--repeats 5 fix the "flaky" parser test --check "cargo test -p parser" --budget $2.50 --against=opus"#,
    );
    assert_eq!(
        request,
        CompareRequest {
            prompt: Some(r#"fix the "flaky" parser test"#.to_string()),
            plan: None,
            session: None,
            a: None,
            against: Some("opus".to_string()),
            repeats: 5,
            check: Some("cargo test -p parser".to_string()),
            budget_usd: Some(2.5),
            examples: None,
            learn: false,
            suite: None,
            classifier: None,
        }
    );
}

#[test]
fn a_classifier_names_the_model_that_classifies_the_prompts() {
    let request = start("fix it --classifier claude-haiku-4-5");
    assert_eq!(request.classifier.as_deref(), Some("claude-haiku-4-5"));
    assert_eq!(request.prompt.as_deref(), Some("fix it"));
    assert_eq!(
        start("--classifier=' openrouter/openai/gpt-5-nano ' --examples 2").classifier,
        Some("openrouter/openai/gpt-5-nano".to_string())
    );
    assert_eq!(start("fix it").classifier, None, "Gate's choice by default");
    assert_eq!(start("fix it --classifier AUTO").classifier, None);
    assert_eq!(CompareRequest::default().classifier, None);
}

#[test]
fn a_suite_picks_where_example_tasks_come_from() {
    let request = start("--suite gate_hard --examples 3");
    assert_eq!(
        (request.suite.as_deref(), request.examples),
        (Some("gate_hard"), Some(3))
    );
    assert_eq!(start("--suite polyglot").suite.as_deref(), Some("polyglot"));
    assert!(
        parse("fix it --suite polyglot").is_err(),
        "a prompt or a suite"
    );
}

#[test]
fn learn_mode_needs_no_value() {
    let request = start("--learn --examples 4");
    assert!(request.learn);
    assert_eq!(request.examples, Some(4));
    assert_eq!(request.repeats, DEFAULT_REPEATS);
    assert!(start("fix it --learn=on").learn);
    assert!(!start("fix it --learn=off").learn);
    assert_eq!(start("fix it --learn").prompt.as_deref(), Some("fix it"));
    assert!(parse("fix it --learn=maybe").is_err());
}

#[test]
fn defaults_apply_when_nothing_is_given() {
    assert_eq!(
        start("--check true"),
        CompareRequest {
            check: Some("true".to_string()),
            ..CompareRequest::default()
        }
    );
    assert_eq!(CompareRequest::default().repeats, DEFAULT_REPEATS);
    assert_eq!(
        start("--plan .aura/plans/x.plan.md").plan.as_deref(),
        Some(".aura/plans/x.plan.md")
    );
    assert_eq!(start("--examples 5 --repeats 1").examples, Some(5));
}

#[test]
fn arms_and_sessions_are_named_by_options() {
    assert_eq!(
        start("fix it --against gate:nodelegate").against.as_deref(),
        Some(NO_DELEGATE)
    );
    let request = start("fix it --a sonnet --b opus");
    assert_eq!(request.a.as_deref(), Some("sonnet"));
    assert_eq!(request.against.as_deref(), Some("opus"));
    assert_eq!(
        start("--session current").session,
        Some(SessionPick::Current)
    );
    assert_eq!(
        start("--session=1700000000000").session,
        Some(SessionPick::Started(1_700_000_000_000))
    );
}

#[test]
fn subcommands_are_whole_arguments_only() {
    assert_eq!(parse("go"), Ok(CompareCommand::Go));
    assert_eq!(parse("cancel"), Ok(CompareCommand::Cancel));
    assert_eq!(parse("dismiss"), Ok(CompareCommand::Dismiss));
    assert_eq!(parse("show"), Ok(CompareCommand::Show(None)));
    assert_eq!(parse("show 2"), Ok(CompareCommand::Show(Some(2))));
    assert_eq!(
        parse("apply other-2"),
        Ok(CompareCommand::Apply("other-2".into()))
    );
    assert_eq!(parse("diff"), Ok(CompareCommand::Diff(None)));
    assert_eq!(
        parse("diff gate-1"),
        Ok(CompareCommand::Diff(Some("gate-1".into())))
    );
    assert!(parse("apply").is_err());
    assert_eq!(
        start("pick the fastest sort").prompt.as_deref(),
        Some("pick the fastest sort")
    );
    assert_eq!(start("-- go").prompt.as_deref(), Some("go"));
    assert_eq!(
        start("show me the parser").prompt.as_deref(),
        Some("show me the parser")
    );
}

#[test]
fn subcommands_ignore_case_except_in_numbers() {
    let cases = [
        ("", CompareCommand::List),
        ("LIST", CompareCommand::List),
        ("Show 3", CompareCommand::Show(Some(3))),
        ("diff A-1", CompareCommand::Diff(Some("a-1".to_string()))),
        ("diff", CompareCommand::Diff(None)),
        ("Apply B-2", CompareCommand::Apply("b-2".to_string())),
        ("stop", CompareCommand::Cancel),
    ];
    for (args, command) in cases {
        assert_eq!(parse(args), Ok(command), "{args:?}");
    }
}

#[test]
fn prompts_are_sent_as_typed() {
    let cases = [
        (
            r#"fix the "flaky" parser test"#,
            r#"fix the "flaky" parser test"#,
        ),
        ("  fix   it  ", "fix it"),
        ("fix it -- --check x  y ", "fix it --check x  y"),
        ("-- go", "go"),
        (r#"fix ünïcödé "tëst ✓""#, r#"fix ünïcödé "tëst ✓""#),
        (r#""--" go"#, r#""--" go"#),
        ("fix it --", "fix it"),
        ("show 0", "show 0"),
        ("go now", "go now"),
        ("it''s", "it''s"),
        ("fix\tit\nnow", "fix it now"),
        ("list all", "list all"),
    ];
    for (args, typed) in cases {
        assert_eq!(
            parse(args),
            Ok(CompareCommand::Start(asked(typed))),
            "{args:?}"
        );
    }
}

#[test]
fn options_are_read_with_or_without_quotes_and_equals() {
    let cases = [
        (
            r#"--check="cargo test" fix it"#,
            CompareRequest {
                check: Some("cargo test".to_string()),
                ..asked("fix it")
            },
        ),
        (
            r#"--budget=$2.50 'quoted prompt' "--not-a-flag""#,
            CompareRequest {
                budget_usd: Some(2.5),
                ..asked(r#"'quoted prompt' "--not-a-flag""#)
            },
        ),
        (
            "--repeats=' 4 ' fix",
            CompareRequest {
                repeats: 4,
                ..asked("fix")
            },
        ),
        (
            "--check --budget 3",
            CompareRequest {
                check: Some("--budget".to_string()),
                ..asked("3")
            },
        ),
        ("--learn=OFF fix", asked("fix")),
    ];
    for (args, request) in cases {
        assert_eq!(parse(args), Ok(CompareCommand::Start(request)), "{args:?}");
    }
}

#[test]
fn options_name_the_work_arms_and_runs() {
    let cases = [
        (
            "--vs opus --runs 2 --tasks 3",
            CompareRequest {
                against: Some("opus".to_string()),
                repeats: 2,
                examples: Some(3),
                ..CompareRequest::default()
            },
        ),
        (
            "--learn=yes --suite gate_hard",
            CompareRequest {
                learn: true,
                suite: Some("gate_hard".to_string()),
                ..CompareRequest::default()
            },
        ),
        (
            r#"--session " 1700000000000 ""#,
            CompareRequest {
                session: Some(SessionPick::Started(1_700_000_000_000)),
                ..CompareRequest::default()
            },
        ),
        (
            "--session=CURRENT",
            CompareRequest {
                session: Some(SessionPick::Current),
                ..CompareRequest::default()
            },
        ),
        (
            "--b gate:nodelegate --a=gate fix",
            CompareRequest {
                a: Some("gate".to_string()),
                against: Some("gate:nodelegate".to_string()),
                ..asked("fix")
            },
        ),
    ];
    for (args, request) in cases {
        assert_eq!(parse(args), Ok(CompareCommand::Start(request)), "{args:?}");
    }
}

#[test]
fn bad_arguments_say_what_is_wrong() {
    let error = |args: &str| parse(args).unwrap_err();
    assert!(error("--repeats 0").contains("1 to 10"));
    assert!(error("--repeats eleven").contains("1 to 10"));
    assert!(error("--budget -1").contains("USD"));
    assert!(error("--colour red x").contains("Unknown option --colour"));
    assert!(error("fix it --check").contains("needs a value"));
    assert!(error("fix it --plan p.plan.md").contains("not more than one"));
    assert!(error("--plan p.plan.md --session current").contains("not more than one"));
    assert!(error("fix it --examples 3").contains("not more than one"));
    assert!(error("--examples none").contains("how many example tasks"));
    assert!(error("--examples 0").contains("how many example tasks"));
    assert!(error("--session yesterday").contains("start time"));
    assert!(error("\"unclosed").contains("Unclosed quote"));
}

#[test]
fn errors_say_what_is_wrong_in_full() {
    let usage = |problem: &str| format!("{problem} {USAGE}");
    let cases = [
        (r#"--a "" fix"#, usage("--a needs a value.")),
        ("--against", usage("--against needs a value.")),
        ("--classifier", usage("--classifier needs a value.")),
        (
            r#"--classifier="" fix"#,
            usage("--classifier needs a value."),
        ),
        ("APPLY", usage("/compare APPLY needs a name.")),
        ("--colour red x", usage("Unknown option --colour.")),
        (
            "--plan p.md --suite s",
            usage("Give a prompt, --plan, --session or --examples, not more than one."),
        ),
        (
            "--examples= 3",
            "--examples takes how many example tasks to run, not ``".to_string(),
        ),
        (
            r#"--examples=" 0""#,
            "--examples takes how many example tasks to run, not ` 0`".to_string(),
        ),
        (
            "fix 'it",
            "Unclosed quote in /compare arguments".to_string(),
        ),
        (
            "--budget $0",
            "--budget takes an amount in USD such as 2.50, not `$0`".to_string(),
        ),
        (
            "--learn=maybe",
            "--learn takes on or off, not `maybe`".to_string(),
        ),
        (
            "--repeats 11",
            "--repeats takes a number from 1 to 10, not `11`".to_string(),
        ),
        (
            "--session yesterday",
            "--session takes `current` or a session's start time, not `yesterday`".to_string(),
        ),
    ];
    for (args, error) in cases {
        assert_eq!(parse(args), Err(error), "{args:?}");
    }
}
