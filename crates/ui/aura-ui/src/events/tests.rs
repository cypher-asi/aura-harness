use super::*;

fn message_at(content: &str, at: Option<u64>) -> MessageData {
    MessageData {
        timestamp_ms: at,
        ..MessageData::new(MessageRole::User, content)
    }
}

fn post_at(text: &str, at_ms: u64) -> ChannelPostView {
    ChannelPostView {
        text: text.into(),
        at_ms,
        ..ChannelPostView::default()
    }
}

/// Each item as `m:<content>` or `p:<text>`.
fn labels(items: &[TimelineItem<'_>]) -> Vec<String> {
    items
        .iter()
        .map(|item| match item {
            TimelineItem::Message(m) => format!("m:{}", m.content),
            TimelineItem::Post(p) => format!("p:{}", p.text),
        })
        .collect()
}

#[test]
fn merge_timeline_orders_by_time_with_messages_first_on_ties() {
    let history = [message_at("a", Some(10)), message_at("c", Some(30))];
    let posts = [post_at("b", 20), post_at("tie", 30), post_at("d", 40)];
    assert_eq!(
        labels(&merge_timeline(&history, &posts)),
        ["m:a", "p:b", "m:c", "p:tie", "p:d"]
    );
}

#[test]
fn merge_timeline_puts_untimed_messages_after_every_post() {
    let history = [message_at("a", Some(10)), message_at("now", None)];
    let posts = [post_at("b", 5), post_at("c", u64::MAX)];
    assert_eq!(
        labels(&merge_timeline(&history, &posts)),
        ["p:b", "m:a", "p:c", "m:now"]
    );
}

#[test]
fn merge_timeline_keeps_either_list_alone_as_is() {
    let history = [message_at("x", None), message_at("y", Some(1))];
    assert_eq!(labels(&merge_timeline(&history, &[])), ["m:x", "m:y"]);
    let posts = [post_at("p", 9), post_at("q", 3)];
    assert_eq!(labels(&merge_timeline(&[], &posts)), ["p:p", "p:q"]);
    assert!(merge_timeline(&[], &[]).is_empty());
}

#[test]
fn a_policy_view_counts_its_tools_by_state() {
    let tool = |name: &str, state| ToolPolicyView {
        name: name.into(),
        state,
        decided_by: String::new(),
    };
    let view = PolicyView {
        tools: vec![
            tool("read_file", ToolPolicyState::On),
            tool("list_files", ToolPolicyState::On),
            tool("write_file", ToolPolicyState::Ask),
            tool("git_push", ToolPolicyState::Off),
        ],
        ..PolicyView::default()
    };
    assert_eq!(view.tool_counts(), (2, 1, 1));
    assert_eq!(ToolPolicyState::Ask.label(), "ask");
}

#[test]
fn test_ui_event_variants() {
    let event = UiEvent::UserMessage {
        agent_id: "aa".to_string(),
        text: "hello".to_string(),
        shown: true,
        images: Vec::new(),
    };
    assert!(matches!(event, UiEvent::UserMessage { .. }));

    let event = UiEvent::Approve("tool-1".to_string());
    assert!(matches!(event, UiEvent::Approve(_)));

    let event = UiEvent::Quit;
    assert!(matches!(event, UiEvent::Quit));
}

#[test]
fn test_ui_command_variants() {
    let cmd = UiCommand::SetStatus("Ready".to_string());
    assert!(matches!(cmd, UiCommand::SetStatus(_)));

    let cmd = UiCommand::CompleteTool {
        id: "1".to_string(),
        result: "ok".to_string(),
        success: true,
    };
    assert!(matches!(cmd, UiCommand::CompleteTool { .. }));
}

#[test]
fn test_message_role_equality() {
    assert_eq!(MessageRole::User, MessageRole::User);
    assert_ne!(MessageRole::User, MessageRole::Assistant);
}

fn member(id: &str, parent: Option<&str>) -> (AgentSummary, Option<String>) {
    (
        AgentSummary {
            id: id.into(),
            depth: 9,
            ..AgentSummary::default()
        },
        parent.map(Into::into),
    )
}

fn order(nested: &[AgentSummary]) -> Vec<(&str, usize)> {
    nested.iter().map(|a| (a.id.as_str(), a.depth)).collect()
}

#[test]
fn nest_members_puts_the_coordinators_agents_beside_it_with_subagents_below() {
    let nested = nest_members(
        "co",
        vec![
            member("a", Some("co")),
            member("a1", Some("a")),
            member("b", Some("co")),
            member("co", None),
            member("a2", Some("a")),
            member("b1", Some("b")),
            member("a1x", Some("a1")),
        ],
    );
    assert_eq!(
        order(&nested),
        vec![
            ("co", 0),
            ("a", 0),
            ("a1", 1),
            ("a1x", 2),
            ("a2", 1),
            ("b", 0),
            ("b1", 1),
        ]
    );
}

#[test]
fn nest_members_counts_orphans_as_the_coordinators_agents() {
    let nested = nest_members(
        "co",
        vec![
            member("x", Some("gone")),
            member("co", Some("someone")),
            member("y", None),
            member("x1", Some("x")),
        ],
    );
    assert_eq!(
        order(&nested),
        vec![("co", 0), ("x", 0), ("x1", 1), ("y", 0)]
    );
}

#[test]
fn nest_members_keeps_members_without_a_coordinator_or_in_a_cycle() {
    let nested = nest_members(
        "co",
        vec![
            member("p", Some("q")),
            member("q", Some("p")),
            member("a", Some("co")),
            member("a1", Some("a")),
        ],
    );
    assert_eq!(
        order(&nested),
        vec![("a", 0), ("a1", 1), ("p", 0), ("q", 1)]
    );
    assert!(nest_members("co", Vec::new()).is_empty());
}

#[test]
fn channel_views_round_trip_through_json() {
    let channel = ChannelSummary {
        id: "ch1".into(),
        slug: "release".into(),
        title: "Release".into(),
        description: "Ship 1.0".into(),
        coordinator_id: "co".into(),
        project: Some(ChannelProjectView {
            id: "p1".into(),
            name: "aura".into(),
            folder: "/code/aura".into(),
        }),
        members: nest_members("co", vec![member("co", None), member("a", Some("co"))]),
        archived: false,
        placement: crate::host::PlacementView::Cloud,
    };
    let json = serde_json::to_string(&channel).unwrap();
    assert_eq!(
        serde_json::from_str::<ChannelSummary>(&json).unwrap(),
        channel
    );

    for author in [
        ChannelPostAuthor::User,
        ChannelPostAuthor::Agent {
            id: "a".into(),
            code: Some("RE1".into()),
            name: "RE1: Fix".into(),
        },
    ] {
        let post = ChannelPostView {
            seq: 3,
            author,
            text: "@RE1 look".into(),
            mentions: vec!["a".into()],
            reply_to: Some(2),
            at_ms: 1_700_000_000_000,
            planner: Some("p".into()),
        };
        let json = serde_json::to_string(&post).unwrap();
        assert_eq!(
            serde_json::from_str::<ChannelPostView>(&json).unwrap(),
            post
        );
    }
}

#[test]
fn prompt_keys_are_stable_trimmed_and_never_a_post_seq() {
    use super::{prompt_key, PROMPT_KEY_BIT};
    assert_eq!(
        prompt_key("fix the parser"),
        prompt_key("  fix the parser\n")
    );
    assert_ne!(prompt_key("fix the parser"), prompt_key("fix the lexer"));
    assert_ne!(prompt_key("a") & PROMPT_KEY_BIT, 0);
    // Pinned: the session and the UI must agree on it.
    assert_eq!(prompt_key(""), 0xcbf2_9ce4_8422_2325 | PROMPT_KEY_BIT);
}

#[test]
fn typed_prompts_leave_out_empty_text_and_inbox_mail() {
    use super::is_typed_prompt;
    assert!(is_typed_prompt("ship it"));
    assert!(!is_typed_prompt("   "));
    assert!(!is_typed_prompt(
        "RE36: Parser done\n(inbox reply 7 from RE36, Parser)\nAll green."
    ));
}
