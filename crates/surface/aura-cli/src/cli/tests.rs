use super::*;
use std::path::PathBuf;
use std::time::Duration;

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("aura").chain(args.iter().copied()))
}

#[test]
fn print_takes_a_prompt_or_reads_stdin() {
    let cli = parse(&["-p", "fix the tests"]).unwrap();
    assert_eq!(cli.print.as_deref(), Some("fix the tests"));
    assert!(cli.command.is_none());

    assert_eq!(parse(&["-p"]).unwrap().print.as_deref(), Some("-"));
    assert_eq!(
        parse(&["--print", "-"]).unwrap().print.as_deref(),
        Some("-")
    );
    assert_eq!(
        parse(&["-p", "--output-format", "json"])
            .unwrap()
            .print
            .as_deref(),
        Some("-")
    );
    assert!(parse(&[]).unwrap().print.is_none());
}

#[test]
fn gate_eval_parses() {
    let eval = |args: &[&str]| match parse(args).unwrap().command {
        Some(Commands::Gate(GateCommand {
            action:
                GateSubcommand::Eval {
                    log,
                    routed,
                    passed,
                    task,
                    json,
                },
        })) => (log, routed, passed, task, json),
        _ => panic!("not gate eval: {args:?}"),
    };
    assert_eq!(eval(&["gate", "eval"]), (None, false, false, None, false));
    assert_eq!(
        eval(&[
            "gate",
            "eval",
            "--log",
            "/tmp/attempts.jsonl",
            "--routed",
            "--passed",
            "--task",
            "swe_bench",
            "--json"
        ]),
        (
            Some(PathBuf::from("/tmp/attempts.jsonl")),
            true,
            true,
            Some("swe_bench".to_string()),
            true
        )
    );
}

/// `aura gate replay` fields: data dir, task, limit, the five switches
/// (thinking, all, dry run, again, criteria), the two models (judge,
/// writer) and jobs.
type ReplayFields = (
    Option<PathBuf>,
    Option<String>,
    Option<usize>,
    (bool, bool, bool, bool, bool),
    (Option<String>, Option<String>),
    usize,
);

fn parse_replay(args: &[&str]) -> ReplayFields {
    match parse(args).unwrap().command {
        Some(Commands::Gate(GateCommand {
            action:
                GateSubcommand::Replay {
                    data_dir,
                    task,
                    limit,
                    thinking,
                    all,
                    judge,
                    dry_run,
                    again,
                    jobs,
                    criteria,
                    writer,
                },
        })) => (
            data_dir,
            task,
            limit,
            (thinking, all, dry_run, again, criteria),
            (judge, writer),
            jobs,
        ),
        _ => panic!("not gate replay: {args:?}"),
    }
}

#[test]
fn gate_replay_defaults() {
    assert_eq!(
        parse_replay(&["gate", "replay"]),
        (
            None,
            None,
            None,
            (false, false, false, false, false),
            (None, None),
            4
        )
    );
}

#[test]
fn gate_replay_writer_requires_criteria() {
    assert!(
        parse(&["gate", "replay", "--writer", "claude/claude-sonnet-5-5"]).is_err(),
        "a writer is named only with --criteria"
    );
}

#[test]
fn gate_replay_parses_every_flag() {
    assert_eq!(
        parse_replay(&[
            "gate",
            "replay",
            "--data-dir",
            "/tmp/data",
            "--task",
            "ansible",
            "--limit",
            "5",
            "--thinking",
            "--all",
            "--judge",
            "claude/claude-opus-5-5",
            "--dry-run",
            "--again",
            "--jobs",
            "2",
            "--criteria",
            "--writer",
            "claude/claude-sonnet-5-5"
        ]),
        (
            Some(PathBuf::from("/tmp/data")),
            Some("ansible".to_string()),
            Some(5),
            (true, true, true, true, true),
            (
                Some("claude/claude-opus-5-5".to_string()),
                Some("claude/claude-sonnet-5-5".to_string())
            ),
            2
        )
    );
}

#[test]
fn a_prompt_naming_a_subcommand_is_still_the_prompt() {
    let cli = parse(&["-p", "hello"]).unwrap();
    assert_eq!(cli.print.as_deref(), Some("hello"));
    assert!(cli.command.is_none());
}

#[test]
fn headless_flags_parse() {
    let cli = parse(&[
        "-p",
        "go",
        "--output-format",
        "stream-json",
        "--resume",
        "abc:1",
        "--allowed-tools",
        "read_file,run_command",
        "--disallowed-tools",
        "write_file",
        "--max-turns",
        "12",
        "--max-time",
        "90s",
        "--max-cost-usd",
        "2.5",
        "-d",
        "/tmp/project",
        "--model",
        "claude/claude-sonnet-4-5",
        "--provider",
        "mock",
        "--mode",
        "plan",
    ])
    .unwrap();
    let h = &cli.headless;
    assert_eq!(h.output_format, Some(OutputFormat::StreamJson));
    assert_eq!(h.resume.as_deref(), Some("abc:1"));
    assert_eq!(h.allowed_tools, ["read_file", "run_command"]);
    assert_eq!(h.disallowed_tools, ["write_file"]);
    assert_eq!(h.max_turns, Some(12));
    assert_eq!(h.max_time, Some(Duration::from_secs(90)));
    assert_eq!(h.max_cost_usd, Some(2.5));
    assert_eq!(h.cwd.as_deref(), Some(std::path::Path::new("/tmp/project")));
    assert_eq!(h.model.as_deref(), Some("claude/claude-sonnet-4-5"));
    assert_eq!(h.provider.as_deref(), Some("mock"));
    assert_eq!(cli.mode, Some(ModeFlag::Plan));
}

#[test]
fn only_bare_aura_and_aura_desktop_open_the_window() {
    let desktop = |args: &[&str]| crate::desktop_launch(&parse(args).unwrap());
    let window = desktop(&["desktop", "--demo"]).expect("aura desktop opens the window");
    assert!(window.demo);
    assert_eq!(window.provider, "anthropic");
    for args in [
        &["--terminal"][..],
        &["--tui", "--mode", "plan"],
        &["run"],
        &["-p", "x"],
        &["runs", "list"],
    ] {
        assert!(desktop(args).is_none(), "{args:?} opened the window");
    }
    assert!(parse(&["--terminal", "-p", "x"]).is_err());
}

#[test]
fn continue_and_resume_exclude_each_other() {
    assert!(parse(&["-p", "x", "-c"]).unwrap().headless.continue_session);
    assert!(parse(&["-p", "x", "--continue", "--resume", "a:1"]).is_err());
}

#[test]
fn headless_flags_require_print() {
    for args in [
        &["--output-format", "json"][..],
        &["--continue"],
        &["--max-turns", "3"],
        &["--cwd", "/tmp"],
        &["--allowed-tools", "read_file"],
    ] {
        assert!(parse(args).is_err(), "{args:?} parsed without -p");
    }
}

#[test]
fn limits_reject_nonsense() {
    assert!(parse(&["-p", "x", "--max-turns", "0"]).is_err());
    assert!(parse(&["-p", "x", "--max-time", "soon"]).is_err());
    assert!(parse(&["-p", "x", "--max-cost-usd", "-1"]).is_err());
    assert!(parse(&["-p", "x", "--output-format", "yaml"]).is_err());
}

#[test]
fn the_hub_can_ask_a_machine_for_its_node_protocol() {
    assert!(matches!(
        parse(&["node", "protocol"]).unwrap().command,
        Some(Commands::Node(NodeCommand {
            action: NodeSubcommand::Protocol
        }))
    ));
}

#[test]
fn run_subcommand_still_owns_its_dir_flag() {
    let cli = parse(&["run", "-d", "/data"]).unwrap();
    let Some(Commands::Run(args)) = cli.command else {
        panic!("expected run");
    };
    assert_eq!(args.dir.as_deref(), Some(std::path::Path::new("/data")));
    assert!(cli.print.is_none());
}

#[test]
fn changelog_previews_unless_asked_to_write() {
    match parse(&["changelog"]).unwrap().command {
        Some(Commands::Changelog(args)) => {
            assert!(!args.write);
            assert!(args.repo.is_none());
        }
        _ => panic!("not the changelog command"),
    }
    match parse(&["changelog", "--write", "--repo", "/r"])
        .unwrap()
        .command
    {
        Some(Commands::Changelog(args)) => {
            assert!(args.write);
            assert_eq!(args.repo, Some(PathBuf::from("/r")));
        }
        _ => panic!("not the changelog command"),
    }
}
