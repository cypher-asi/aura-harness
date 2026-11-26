use super::*;

#[test]
fn test_chat_shows_working_indicator_until_reply_streams() {
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    submit_message(&mut app, "hi");
    let rows = render_rows(&mut app, 120);
    let chat = rows.join("\n");
    assert!(!chat.contains("AURA  "), "{chat}");
    let working = rows.iter().find(|r| r.contains("⬢ Thinking…")).unwrap();
    let prompt = rows.iter().find(|r| r.contains("→ hi")).unwrap();
    assert_eq!(
        working.chars().position(|c| c == '⬢'),
        prompt.chars().position(|c| c == '→'),
        "the hexagon starts at the prompt's column: {chat}"
    );

    app.process_command(UiCommand::StartStreaming);
    assert!(render_rows(&mut app, 120).join("\n").contains("Thinking…"));

    app.process_command(UiCommand::AppendText("hello".to_string()));
    assert!(!render_rows(&mut app, 120).join("\n").contains("Thinking…"));

    app.process_command(UiCommand::Complete);
    assert!(app.working_label().is_none());
}

/// The chat row closing a turn whose reply came from `model` with
/// `usage`, drawn `width` columns wide, from its `[` to its `]`.
fn reply_footer_row(model: &str, usage: Option<CallUsage>, width: u16) -> String {
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    submit_message(&mut app, "hi");
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("hello".to_string()));
    app.process_command(UiCommand::SetReplyModel {
        model: model.to_string(),
        usage,
        sources: Vec::new(),
    });
    app.process_command(UiCommand::Complete);

    let rows = render_rows(&mut app, width);
    let prompt = rows.iter().position(|r| r.contains("→ hi")).unwrap();
    assert!(!rows[prompt].contains(" · "), "{:?}", rows[prompt]);
    let reply = rows.iter().position(|r| r.contains("hello")).unwrap();
    // A blank separator row sits between the reply and its footer, so the
    // footer is the first row below the reply that carries a bracket.
    let footer = rows[reply + 1..]
        .iter()
        .find(|r| r.contains('['))
        .unwrap_or_else(|| panic!("no footer after reply: {rows:?}"));
    let start = footer.find('[').unwrap_or_else(|| panic!("{footer:?}"));
    let end = footer.find(']').unwrap();
    footer[start..=end].to_string()
}

#[test]
fn test_reply_footer_shows_model_tokens_and_cost_after_the_turn() {
    let usage = CallUsage {
        tokens: 12_400,
        cost_usd: Some(0.0418),
        unpriced_calls: 0,
    };
    let footer = reply_footer_row("claude-opus-4-7", Some(usage), 200);
    assert_eq!(footer, "[claude-opus-4-7 · 12.4k tok · $0.042]");
}

#[test]
fn test_reply_footer_marks_a_partly_priced_turn() {
    let usage = CallUsage {
        tokens: 12_400,
        cost_usd: Some(0.0418),
        unpriced_calls: 2,
    };
    let footer = reply_footer_row("claude-opus-4-7 · glm-9", Some(usage), 200);
    assert_eq!(footer, "[claude-opus-4-7 · glm-9 · 12.4k tok · $0.042+?]");
    let unpriced = CallUsage {
        tokens: 900,
        cost_usd: None,
        unpriced_calls: 2,
    };
    assert_eq!(
        reply_footer_row("glm-9", Some(unpriced), 200),
        "[glm-9 · 900 tok]",
        "nothing priced shows no cost, as before"
    );
}

#[test]
fn test_reply_footer_without_usage_or_cost() {
    assert_eq!(reply_footer_row("qwen3:4b", None, 200), "[qwen3:4b]");

    let local = CallUsage {
        tokens: 900,
        cost_usd: None,
        unpriced_calls: 0,
    };
    assert_eq!(
        reply_footer_row("qwen3:4b", Some(local), 200),
        "[qwen3:4b · 900 tok]"
    );
}

#[test]
fn test_reply_footer_drops_cost_then_tokens_when_narrow() {
    use super::super::segments::{reply_footer, reply_meta};

    let usage = CallUsage {
        tokens: 1_250_000,
        cost_usd: Some(3.5),
        unpriced_calls: 0,
    };
    let meta = reply_meta(Some("claude-opus-4-7"), Some(usage), &[]);
    let footer = |width| {
        reply_footer(&meta, width, &test_theme()).map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
    };
    assert_eq!(footer(50).unwrap(), "[claude-opus-4-7 · 1.2M tok · $3.50]");
    assert_eq!(footer(30).unwrap(), "[claude-opus-4-7 · 1.2M tok]");
    assert_eq!(footer(20).unwrap(), "[claude-opus-4-7]");
    assert_eq!(footer(12).unwrap(), "[claude-op…]");
    assert!(footer(3).is_none());
    assert!(reply_footer(&[], 50, &test_theme()).is_none());
}

#[test]
fn test_reply_meta_formats_tokens_and_cost() {
    use super::super::segments::reply_meta;

    let meta = |tokens, cost_usd| {
        let usage = CallUsage {
            tokens,
            cost_usd,
            unpriced_calls: 0,
        };
        reply_meta(Some("m"), Some(usage), &[]).join(" · ")
    };
    assert_eq!(meta(842, Some(0.004)), "m · 842 tok · <$0.01");
    assert_eq!(meta(12_449, Some(0.0418)), "m · 12.4k tok · $0.042");
    assert_eq!(meta(240_000, Some(1.234)), "m · 240k tok · $1.23");
    assert_eq!(meta(3_400_000, None), "m · 3.4M tok");
    assert_eq!(reply_meta(None, None, &[]), Vec::<String>::new());
}

#[test]
fn test_working_indicator_names_running_tool() {
    use crate::events::{ToolData, UiCommand};

    let mut app = crate::App::new();
    submit_message(&mut app, "read it");
    app.process_command(UiCommand::ShowTool(ToolData {
        id: "t1".to_string(),
        name: "read_file".to_string(),
        args: String::new(),
    }));
    assert_eq!(app.working_label().as_deref(), Some("running read_file"));

    app.process_command(UiCommand::CompleteTool {
        id: "t1".to_string(),
        result: "no such file".to_string(),
        success: false,
    });
    assert_eq!(app.working_label().as_deref(), Some("thinking"));
}

#[test]
fn test_reply_footer_says_who_paid_for_the_tokens() {
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    submit_message(&mut app, "hi");
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("hello".to_string()));
    app.process_command(UiCommand::SetReplyModel {
        model: "claude-sonnet-4-6 · claude-haiku-4.5".to_string(),
        usage: Some(split_usage()),
        sources: split_sources().to_vec(),
    });
    app.process_command(UiCommand::Complete);

    let rows = render_rows(&mut app, 140);
    let footer = rows
        .iter()
        .position(|r| r.contains("[claude-sonnet-4-6 · claude-haiku-4.5"))
        .expect("the model footer");
    assert!(
        !rows[footer].contains("27.9k tok"),
        "the sign-ins' shares replace the totals: {:?}",
        &rows[footer]
    );
    assert!(
        rows.get(footer + 1).is_none_or(|r| !r.contains('[')),
        "the footer is one line: {:?}",
        &rows[footer..]
    );
}

#[test]
fn test_reply_meta_lists_each_sign_ins_share() {
    use crate::events::SourceUsage;

    let sources = split_sources();
    let usage = split_usage();
    assert_eq!(
        super::super::segments::reply_meta(Some("m"), Some(usage), &sources).join(" · "),
        "m · via Claude plan 25.1k tok · OpenRouter 2.8k tok <$0.01"
    );
    let partly = [
        sources[0].clone(),
        SourceUsage {
            unpriced_calls: 1,
            ..sources[1].clone()
        },
    ];
    assert_eq!(
        super::super::segments::reply_meta(Some("m"), Some(usage), &partly).join(" · "),
        "m · via Claude plan 25.1k tok · OpenRouter 2.8k tok <$0.01+?"
    );
}

/// A reply's 27.9k tokens, paid partly by a plan and partly by OpenRouter.
fn split_usage() -> CallUsage {
    CallUsage {
        tokens: 27_900,
        cost_usd: Some(0.004),
        unpriced_calls: 0,
    }
}

/// The shares of [`split_usage`] by sign-in.
fn split_sources() -> [crate::events::SourceUsage; 2] {
    use crate::events::SourceUsage;

    [
        SourceUsage {
            label: "Claude plan".to_string(),
            tokens: 25_100,
            cost_usd: None,
            unpriced_calls: 0,
        },
        SourceUsage {
            label: "OpenRouter".to_string(),
            tokens: 2_800,
            cost_usd: Some(0.004),
            unpriced_calls: 0,
        },
    ]
}

#[test]
fn test_reply_footer_names_a_lone_sign_in_without_repeating_its_totals() {
    use super::super::segments::reply_meta;
    use crate::events::SourceUsage;

    let meta = reply_meta(
        Some("claude-opus-5-5 ×28 · high thinking"),
        Some(CallUsage {
            tokens: 1_600_000,
            cost_usd: Some(0.998),
            unpriced_calls: 0,
        }),
        &[SourceUsage {
            label: "Claude plan".to_string(),
            tokens: 1_600_000,
            cost_usd: Some(0.998),
            unpriced_calls: 0,
        }],
    );
    assert_eq!(
        meta.join(" · "),
        "claude-opus-5-5 ×28 · high thinking · 1.6M tok · $0.998 · via Claude plan"
    );
}
