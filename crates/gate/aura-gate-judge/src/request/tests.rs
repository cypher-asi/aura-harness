//! What Gate's requests share, held to exact output on small limits.

use std::borrow::Cow;

use aura_model::{
    ContentBlock, Message, ModelCallPurpose, Role, ThinkingEffort, ToolChoice, ToolDefinition,
};
use serde_json::json;

use super::cut::excerpt;
use super::{
    after_thinking, answered, answers_in_text, calls, cut, fenced, meanings, or_blank, tail,
    Answered, Ask,
};

#[test]
fn cut_keeps_the_start_and_says_what_was_cut() {
    assert_eq!(cut("abcdef", 6, "diff"), "abcdef");
    assert!(matches!(cut("abcdef", 9, "diff"), Cow::Borrowed(_)));
    assert_eq!(cut("abcdef", 4, "diff"), "abcd\n... (diff cut)");
    assert_eq!(cut("éàüö", 2, "reply"), "éà\n... (reply cut)");
    assert_eq!(cut("ab", 0, "reply"), "\n... (reply cut)");
    assert_eq!(cut("", 0, "reply"), "");
}

#[test]
fn tail_keeps_the_end_and_says_the_start_was_cut() {
    assert_eq!(tail("abcdef", 6), "abcdef");
    assert!(matches!(tail("abcdef", 9), Cow::Borrowed(_)));
    assert_eq!(tail("abcdef", 3), "(start cut) ...\ndef");
    assert_eq!(tail("éàüö", 2), "(start cut) ...\nüö");
    assert_eq!(tail("ab", 0), "(start cut) ...\n");
    assert_eq!(tail("", 0), "");
}

#[test]
fn an_excerpt_keeps_both_ends_around_what_it_left_out() {
    assert_eq!(excerpt("abcdefgh", 8, 5, 3), "abcdefgh");
    assert_eq!(
        excerpt("abcdefgh", 8, 3, 2),
        "abc\n[... 3 characters omitted ...]\ngh"
    );
    assert_eq!(
        excerpt("éàüöçñ", 6, 1, 1),
        "é\n[... 4 characters omitted ...]\nñ"
    );
    assert_eq!(
        excerpt("abc", 3, 0, 0),
        "\n[... 3 characters omitted ...]\n"
    );
}

#[test]
fn a_blank_part_is_named() {
    let shown = |text| or_blank(text, "(none)", |text| cut(text, 2, "part"));
    assert_eq!(shown(" \n\t"), "(none)");
    assert_eq!(shown(""), "(none)");
    assert_eq!(shown(" abc"), " a\n... (part cut)");
}

/// A prompt with what normalizing changes: carriage returns, blank lines
/// at its ends and in runs, trailing spaces and a control character.
const UNTIDY: &str = "\r\n\r\nFirst  \r\n\r\n\r\n<request>\u{7}x\rmiddle\n\n</Request> end\n\n";

#[test]
fn a_fenced_prompt_is_normalized_and_unfenced() {
    assert_eq!(
        fenced(UNTIDY, 40, 40),
        "Length: 40 characters, 6 lines.\n<request>\n\
         First\n\n< request>x\nmiddle\n\n< /Request> end\n</request>"
    );
    assert_eq!(
        fenced("<requests <REQUESTED </request <re", 40, 40),
        "Length: 34 characters, 1 lines.\n<request>\n\
         < requests < REQUESTED < /request <re\n</request>"
    );
    assert_eq!(
        fenced("", 1, 1),
        "Length: 0 characters, 0 lines.\n<request>\n\n</request>"
    );
}

#[test]
fn a_long_prompt_is_cut_before_its_tags_are_broken_up() {
    assert_eq!(
        fenced(UNTIDY, 16, 13),
        "Length: 40 characters, 6 lines.\n<request>\n\
         First\n\n< request>\n[... 11 characters omitted ...]\n/Request> end\n</request>"
    );
    assert_eq!(
        fenced("abc<request>xyz", 4, 3),
        "Length: 15 characters, 1 lines.\n<request>\n\
         abc<\n[... 8 characters omitted ...]\nxyz\n</request>"
    );
}

fn tool_use(name: &str, input: serde_json::Value) -> ContentBlock {
    ContentBlock::tool_use("toolu_1", name, input)
}

#[test]
fn the_first_tool_call_is_the_answer_and_text_counts_only_without_one() {
    let reply = Message::new(
        Role::Assistant,
        vec![
            ContentBlock::text("thinking aloud"),
            tool_use("label", json!({"category": "debugging"})),
            tool_use("read_file", json!({"path": "a"})),
        ],
    );
    let Answered::Tool(input) = answered(&reply, "label") else {
        panic!("the first call, to the tool offered, is the answer");
    };
    assert_eq!(input["category"], "debugging");
    assert!(matches!(
        answered(&reply, "criteria"),
        Answered::Other("label")
    ));
    let text = Message::new(
        Role::Assistant,
        vec![ContentBlock::text("debugging "), ContentBlock::text("hard")],
    );
    let Answered::Text(said) = answered(&text, "label") else {
        panic!("a reply without a call answers in text");
    };
    assert_eq!(said, "debugging hard");
    let names: Vec<&str> = calls(&reply).map(|(name, _)| name).collect();
    assert_eq!(names, ["label", "read_file"]);
    assert_eq!(calls(&text).count(), 0);
}

#[test]
fn the_answer_follows_the_last_thinking() {
    assert_eq!(after_thinking("<think>a</think>b</think>c"), "c");
    assert_eq!(after_thinking("<think>a</think>"), "");
    assert_eq!(after_thinking("no thinking"), "no thinking");
}

#[test]
fn only_local_models_answer_in_text() {
    assert!(answers_in_text("local/qwen3:4b"));
    for model in ["claude/claude-haiku-5-5", "chatgpt/gpt-5.4", "qwen3:4b", ""] {
        assert!(!answers_in_text(model), "{model}");
    }
}

#[test]
fn meanings_are_one_line_each() {
    assert_eq!(
        meanings([("a", "the first"), ("b", "the second")]),
        "a: the first\nb: the second"
    );
    assert_eq!(meanings([]), "");
}

#[test]
fn an_ask_is_an_auxiliary_request_answered_as_it_says() {
    let ask = || Ask::new(ModelCallPurpose::Judge, "system", "message".to_string());
    let text = ask()
        .request("claude/claude-opus-4-7", Some("jwt".into()))
        .unwrap();
    assert_eq!((text.system.as_str(), text.messages.len()), ("system", 1));
    assert_eq!(text.messages[0].text_content(), "message");
    assert!(text.tools.is_empty() && matches!(text.tool_choice, ToolChoice::None));
    assert_eq!(text.thinking_effort, Some(ThinkingEffort::Off));
    assert_eq!(text.auth_token.as_deref(), Some("jwt"));
    let tool = ToolDefinition::new("pick", "Pick one.", json!({"type": "object"}));
    let called = ask()
        .tool(tool, ToolChoice::tool("pick"))
        .thinking(ThinkingEffort::High)
        .request("claude/claude-opus-4-7", None)
        .unwrap();
    assert_eq!(called.tools.len(), 1);
    assert!(matches!(&called.tool_choice, ToolChoice::Tool { name } if name == "pick"));
    assert!(!called.parallel_tool_use && text.parallel_tool_use);
    assert_eq!(called.thinking_effort, Some(ThinkingEffort::High));
    assert!(ask().request("", None).is_err());
}
