
use super::*;

fn call(input: u64, read: u64, write: u64, cost: Option<f64>) -> CallStats {
    CallStats {
        model: "aura-claude-sonnet-4-6".to_string(),
        input_tokens: input,
        cache_read_tokens: read,
        cache_write_tokens: write,
        output_tokens: 100,
        cost_usd: cost,
        unpriced: false,
        context_limit: Some(200_000),
    }
}

/// A call to a model with no known price.
fn unpriced_call() -> CallStats {
    CallStats {
        model: "openrouter/z-ai/glm-9".to_string(),
        unpriced: true,
        ..call(10, 0, 0, None)
    }
}

#[test]
fn a_partly_priced_session_reads_as_a_part() {
    let mut hud = Hud::default();
    hud.stats.add(&call(10, 0, 0, Some(0.05)));
    assert_eq!(hud.stats.cost_label().as_deref(), Some("$0.050"));
    hud.stats.add(&unpriced_call());
    hud.stats.add(&unpriced_call());
    assert_eq!(hud.stats.unpriced_calls, 2);
    assert_eq!(hud.stats.cost_label().as_deref(), Some("$0.050+?"));
    let report = hud.cost_report();
    assert!(
        report.contains("about $0.050+? at API list prices, leaving out 2 calls"),
        "{report}"
    );
}

#[test]
fn local_calls_are_free_not_unpriced() {
    let mut stats = SessionStats::default();
    stats.add(&call(10, 0, 0, Some(0.05)));
    stats.add(&CallStats {
        model: "local/qwen3".to_string(),
        ..call(10, 0, 0, None)
    });
    assert_eq!(stats.cost_label().as_deref(), Some("$0.050"));
}

#[test]
fn restored_totals_keep_their_unpriced_calls() {
    let mut stats = SessionStats::default();
    stats.restore(&SessionTotals {
        calls: 3,
        cost_usd: Some(0.5),
        unpriced_calls: 1,
        ..SessionTotals::default()
    });
    assert_eq!(stats.cost_label().as_deref(), Some("$0.500+?"));
    stats.restore(&SessionTotals {
        calls: 3,
        unpriced_calls: 3,
        ..SessionTotals::default()
    });
    assert_eq!(stats.cost_label(), None, "nothing priced");
}

#[test]
fn models_are_counted_once_and_kept_on_restore() {
    let mut stats = SessionStats::default();
    stats.add(&call(10, 0, 0, None));
    stats.add(&CallStats {
        model: "aura-claude-haiku-4-5".to_string(),
        ..call(10, 0, 0, None)
    });
    stats.add(&call(10, 0, 0, None));
    assert_eq!(
        stats.models,
        ["aura-claude-sonnet-4-6", "aura-claude-haiku-4-5"]
    );

    stats.restore(&SessionTotals {
        calls: 7,
        models: vec!["claude/claude-opus-4-7".to_string()],
        ..SessionTotals::default()
    });
    assert_eq!(stats.models, ["claude/claude-opus-4-7"]);
}

#[test]
fn totals_and_rates() {
    let mut stats = SessionStats::default();
    stats.add(&call(1_000, 0, 9_000, Some(0.05)));
    stats.add(&call(500, 9_000, 500, Some(0.01)));

    assert_eq!(stats.calls, 2);
    assert_eq!(stats.context_tokens, 10_000);
    assert_eq!(stats.context_percent(), Some(5));
    assert_eq!(stats.cache_hit_percent(), Some(45));
    assert!((stats.cost_usd - 0.06).abs() < 1e-9);
    assert!(stats.priced);
}

#[test]
fn sources_report_sums_each_sign_in_across_turns() {
    let source = |label: &str, tokens, cost_usd| SourceUsage {
        label: label.to_string(),
        tokens,
        cost_usd,
        unpriced_calls: 0,
    };
    let first = [
        source("Claude plan", 20_000, None),
        source("OpenRouter", 1_000, Some(0.5)),
    ];
    let second = [source("OpenRouter", 2_000, Some(0.25))];
    let report = sources_report([&first[..], &second[..]]);
    assert!(report.starts_with("\nBy sign-in:"), "{report}");
    let claude = report.lines().find(|l| l.contains("Claude plan")).unwrap();
    assert!(
        claude.contains("20.0k tok") || claude.contains("20k tok"),
        "{report}"
    );
    let openrouter = report.lines().find(|l| l.contains("OpenRouter")).unwrap();
    assert!(
        openrouter.contains("3.0k tok") || openrouter.contains("3k tok"),
        "{report}"
    );
    assert!(openrouter.contains("$0.75"), "{report}");
    assert_eq!(sources_report(std::iter::empty::<&[SourceUsage]>()), "");
}

#[test]
fn sources_report_marks_a_sign_in_with_unpriced_calls() {
    let turn = [SourceUsage {
        label: "OpenRouter".to_string(),
        tokens: 1_000,
        cost_usd: Some(0.5),
        unpriced_calls: 1,
    }];
    let report = sources_report([&turn[..]]);
    assert!(report.contains("$0.500+?"), "{report}");
}

#[test]
fn unpriced_calls_say_so() {
    let mut hud = Hud::default();
    hud.stats.add(&call(10, 0, 0, None));
    assert!(hud.cost_report().contains("no priced calls"));
}

#[test]
fn context_report_lists_instruction_files() {
    let file = |path: &str, bytes, globs: Option<&str>, status| InstructionFileView {
        path: path.to_string(),
        bytes,
        globs: globs.map(str::to_string),
        status,
    };
    let mut hud = Hud::default();
    assert!(!hud.context_report().contains("Project instructions"));

    hud.instructions = Some(Vec::new());
    assert!(hud.context_report().contains("none found (/init"));

    hud.instructions = Some(vec![
        file("~/p/AGENTS.md", 4_000, None, InstructionStatus::Included),
        file(
            "~/p/.cursor/rules/rust.mdc",
            400,
            Some("**/*.rs"),
            InstructionStatus::Included,
        ),
        file(
            "~/p/.cursor/rules/db.mdc",
            0,
            None,
            InstructionStatus::OnRequest,
        ),
        file("/AGENTS.md", 0, None, InstructionStatus::Omitted),
    ]);
    let report = hud.context_report();
    assert!(
        report.contains("Project instructions (~1.1k tokens):"),
        "{report}"
    );
    assert!(report.contains("~/p/AGENTS.md  1.0k"), "{report}");
    assert!(report.contains("rust.mdc  100 for **/*.rs"), "{report}");
    assert!(report.contains("db.mdc  read when relevant"), "{report}");
    assert!(
        report.contains("/AGENTS.md  left out (size limit)"),
        "{report}"
    );
}

#[test]
fn todo_progress_skips_cancelled_items() {
    let todo = |content: &str, state| TodoView {
        content: content.to_string(),
        state,
        assignee: None,
        task: None,
    };
    let hud = Hud {
        todos: vec![
            todo("a", TodoState::Completed),
            todo("b", TodoState::Cancelled),
            todo("c", TodoState::Pending),
            todo("d", TodoState::InProgress),
        ],
        ..Hud::default()
    };
    assert_eq!(hud.todo_progress(), (1, 3));
    assert_eq!(hud.current_todo().map(|t| t.content.as_str()), Some("d"));
}
