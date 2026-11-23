//! The criteria request, the criteria read out of its reply, and how
//! the worker and the reviewer are shown them.

use aura_model::{ContentBlock, Message, Role, ToolChoice};
use serde_json::{json, Value};

use super::answer::{criteria_in, MAX_TEXT};
use super::review::review_section;
use super::{
    checked, none_because, request, tally, worker_note, Criteria, Criterion, Kind, Status,
    CRITERIA_TOOL, EXACTING_ITEMS, MAX_ITEMS, SYSTEM_PROMPT,
};

const SONNET: &str = "claude/claude-sonnet-5-5";

fn called(input: Value) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::tool_use("toolu_1", CRITERIA_TOOL, input)],
    )
}

/// The request of `swe_bench_pro_hard/ansible-b748edea`, whose exact
/// demands sit in the middle of a long prompt.
fn multipart_prompt() -> String {
    format!(
        "Your task is to implement the changes necessary to satisfy the following Pull \
         Request (PR) description:\n\n<pr_description>\n# Title\nLack of a public helper to \
         build multipart/form-data request bodies\n{}\n- The overall assembled body MUST be \
         byte-identical (after boundary substitution) to the multipart body produced by \
         Python's standard-library email.mime.multipart.MIMEMultipart('form-data') \
         serialized with email.policy.HTTP on Python 3.\n{}\n</pr_description>",
        "- requirement text\n".repeat(700),
        "Constraints: run commands with sh .swe/run.\n".repeat(100),
    )
}

#[test]
fn the_writer_reads_the_middle_of_a_long_request() {
    let prompt = multipart_prompt();
    let classifier = crate::classify::request("claude/claude-haiku-4-5", &prompt, None).unwrap();
    assert!(
        !classifier.messages[0]
            .text_content()
            .contains("byte-identical"),
        "the classifier does not read that far"
    );
    let built = request(SONNET, &prompt, Some("jwt".into())).unwrap();
    let sent = built.messages[0].text_content();
    assert!(sent.contains("byte-identical"), "{sent}");
    assert!(!sent.contains("characters omitted"), "{sent}");
    assert_eq!(built.system, SYSTEM_PROMPT);
    assert_eq!(built.auth_token.as_deref(), Some("jwt"));
    assert_eq!(built.tools[0].name, CRITERIA_TOOL);
    assert_eq!(
        built.tools[0].input_schema["properties"]["items"]["items"]["properties"]["kind"]["enum"],
        json!(Kind::ALL.map(Kind::as_str))
    );
    assert!(sent.ends_with("Do not carry it out."), "{sent}");
    assert!(request("", "hi", None).is_err());
}

#[test]
fn the_tool_is_forced_only_where_the_model_takes_that() {
    let choice = |model: &str| request(model, "add a flag", None).unwrap().tool_choice;
    assert!(
        matches!(choice(SONNET), ToolChoice::Auto),
        "sonnet 5.5 rejects a forced tool"
    );
    assert!(matches!(choice("claude/claude-opus-5-5"), ToolChoice::Auto));
    for model in ["claude/claude-sonnet-4-6", "chatgpt/gpt-5.4"] {
        assert!(
            matches!(choice(model), ToolChoice::Tool { name } if name == CRITERIA_TOOL),
            "{model}"
        );
    }
    let built = request(SONNET, "add a flag", None).unwrap();
    assert_eq!(
        built.tools[0].name, CRITERIA_TOOL,
        "the tool is still offered"
    );
    let prose = Message::assistant(
        "Here are the criteria:\n{\"items\": [{\"text\": \"adds --dry-run\", \
         \"kind\": \"interface\"}], \"strict\": false}",
    );
    assert_eq!(
        criteria_in(&prose).map(|c| c.items.len()),
        Some(1),
        "an answer in text counts"
    );
}

#[test]
fn a_local_model_answers_in_text() {
    let built = request("local/qwen3:4b", "add a flag", None).unwrap();
    assert!(built.tools.is_empty());
    assert!(matches!(built.tool_choice, ToolChoice::None));
    let reply = Message::assistant(
        "<think>{\"items\": []}</think>\n{\"items\": [{\"text\": \"adds --dry-run\", \
         \"kind\": \"interface\"}], \"strict\": false}",
    );
    let criteria = criteria_in(&reply).unwrap();
    assert_eq!(criteria.items.len(), 1);
    assert_eq!(criteria.items[0].kind, Kind::Interface);
}

#[test]
fn the_tool_call_is_the_answer() {
    let criteria = criteria_in(&called(json!({
        "items": [
            {"text": "prepare_multipart is importable from ansible.module_utils.urls", "kind": "interface"},
            {
                "text": "  The body is byte-identical\n to MIMEMultipart's  ",
                "kind": "Reference_Match",
                "reference": "stdlib MIMEMultipart with email.policy.HTTP",
            },
            {"text": "the body is byte-identical to MIMEMultipart's", "kind": "exact_output"},
            {"text": "", "kind": "behaviour"},
            {"text": "raises", "kind": "panic"},
            {"kind": "error"},
        ],
        "strict": true,
    })))
    .unwrap();
    assert!(criteria.strict);
    assert_eq!(
        criteria.items,
        [
            Criterion {
                text: "prepare_multipart is importable from ansible.module_utils.urls".to_string(),
                kind: Kind::Interface,
                reference: None,
            },
            Criterion {
                text: "The body is byte-identical to MIMEMultipart's".to_string(),
                kind: Kind::ReferenceMatch,
                reference: Some("stdlib MIMEMultipart with email.policy.HTTP".to_string()),
            },
        ],
        "blank, unknown and repeated items are dropped"
    );
}

#[test]
fn a_reply_without_criteria_gives_none() {
    let other = Message::new(
        Role::Assistant,
        vec![ContentBlock::tool_use(
            "toolu_1",
            "read_file",
            json!({"path": "a"}),
        )],
    );
    for (reply, why) in [
        (
            called(json!({"items": [], "strict": false})),
            "it listed no criteria",
        ),
        (
            called(json!({"strict": true})),
            "its answer holds only strict, not a list of items",
        ),
        (
            called(json!({"items": 3, "strict": true})),
            "its answer holds `items` that is a number, not a list of items",
        ),
        (
            called(json!({"items": [
                {"text": "a", "kind": "style"},
                {"text": "b", "kind": "advice"},
                {"kind": "style"},
            ]})),
            "none of its 3 items has a text and a kind it was given (kinds: advice, style)",
        ),
        (
            Message::assistant("I'll start by reading"),
            "its reply called no tool and holds no JSON object",
        ),
        (Message::assistant(" "), "its reply was empty"),
        (other, "it called `read_file`, not the criteria tool"),
    ] {
        assert_eq!(criteria_in(&reply), None);
        assert_eq!(none_because(&reply), why);
    }
}

/// A long nested argument sometimes comes back as a string of JSON,
/// and a kind in another spelling.
#[test]
fn an_answer_written_as_a_string_of_json_is_read() {
    let items = json!([
        {"text": "Raises ValueError on a malformed size", "kind": "error"},
        {"text": "Keeps the tags of the value", "kind": "behavior"},
        {"text": "Prints `1.5 KB`", "kind": "exact-output"},
    ]);
    let expected = |strict| Criteria {
        items: vec![
            Criterion {
                text: "Raises ValueError on a malformed size".to_string(),
                kind: Kind::Error,
                reference: None,
            },
            Criterion {
                text: "Keeps the tags of the value".to_string(),
                kind: Kind::Behaviour,
                reference: None,
            },
            Criterion {
                text: "Prints `1.5 KB`".to_string(),
                kind: Kind::ExactOutput,
                reference: None,
            },
        ],
        strict,
    };
    let as_string = called(json!({"items": items.to_string(), "strict": "true"}));
    assert_eq!(criteria_in(&as_string), Some(expected(true)));
    let whole = called(Value::String(
        json!({"items": items, "strict": false}).to_string(),
    ));
    assert_eq!(criteria_in(&whole), Some(expected(false)));
    assert_eq!(criteria_in(&called(items)), Some(expected(false)));
    assert_eq!(
        none_because(&called(json!({"items": "see above"}))),
        "its answer holds `items` that is a string, not a list of items"
    );
}

#[test]
fn at_most_twelve_are_kept() {
    let items: Vec<Value> = (0..20)
        .map(|i| json!({"text": format!("criterion {i}"), "kind": "behaviour"}))
        .collect();
    let criteria = criteria_in(&called(json!({"items": items, "strict": false}))).unwrap();
    assert_eq!(criteria.items.len(), MAX_ITEMS);
    assert_eq!(criteria.items[11].text, "criterion 11");
}

fn of(kinds: &[Kind], strict: bool) -> Criteria {
    Criteria {
        items: kinds
            .iter()
            .enumerate()
            .map(|(i, kind)| Criterion {
                text: format!("c{i}"),
                kind: *kind,
                reference: None,
            })
            .collect(),
        strict,
    }
}

#[test]
fn strict_or_many_exacting_criteria_make_work_exacting() {
    assert!(of(&[Kind::Behaviour], true).exacting());
    assert!(!of(&[Kind::ExactOutput; EXACTING_ITEMS - 1], false).exacting());
    let mut many = vec![Kind::ExactOutput; EXACTING_ITEMS - 2];
    many.extend([Kind::ReferenceMatch, Kind::ReferenceMatch, Kind::Interface]);
    assert!(of(&many, false).exacting());
}

#[test]
fn the_list_numbers_each_criterion_with_its_kind_and_reference() {
    let mut criteria = of(&[Kind::Interface, Kind::ReferenceMatch], false);
    criteria.items[1].reference = Some("MIMEMultipart".to_string());
    assert_eq!(
        criteria.listed(),
        "1. [interface] c0\n2. [matches a reference] c1 (reference: MIMEMultipart)"
    );
    assert!(worker_note(&criteria).ends_with(&criteria.listed()));
    let section = review_section(&criteria);
    assert!(section.contains(&criteria.listed()), "{section}");
    assert!(
        section.contains("CRITERION <number>: UNTESTED"),
        "{section}"
    );
}

#[test]
fn the_reviewers_lines_give_each_criterion_its_status() {
    let criteria = of(&[Kind::Interface, Kind::ReferenceMatch, Kind::Error], false);
    let findings = "CRITERION 1: MET, test_wrong_type\n\
                    - criterion 2: untested - only inline parts are compared\n\
                    CRITERION 2: MET\n\
                    CRITERION 9: UNMET\n\
                    1. The base64 parts lack the blank line before the boundary.";
    let statuses: Vec<Option<Status>> = checked(&criteria, findings)
        .into_iter()
        .map(|c| c.status)
        .collect();
    assert_eq!(statuses, [Some(Status::Met), Some(Status::Untested), None]);
}

#[test]
fn the_tally_counts_the_met_of_all_and_names_the_rest() {
    let criteria = of(&[Kind::Interface; 5], false);
    let findings = "CRITERION 1: MET\nCRITERION 2: MET\nCRITERION 3: UNMET\nCRITERION 4: UNTESTED";
    assert_eq!(
        tally(&checked(&criteria, findings)),
        ", criteria 2/5 met, 1 unmet, 1 untested"
    );
    assert_eq!(
        tally(&checked(&criteria, "CRITERION 5: MET")),
        ", criteria 1/5 met"
    );
    assert_eq!(tally(&[]), "");
}

#[test]
fn a_criterion_line_may_be_marked_and_its_first_readable_status_counts() {
    let criteria = of(&[Kind::Interface, Kind::Behaviour, Kind::Error], false);
    let findings = "**CRITERION 1: MET**\n\
                    - criterion #2 :UNMET, it prints `1.5KB`\n\
                    CRITERION 3: maybe\n\
                    CRITERION 3: UNTESTED\n\
                    CRITERION 0: MET\n\
                    CRITERION 1 2: UNMET\n\
                    1. CRITERION 2: MET";
    let statuses: Vec<Option<Status>> = checked(&criteria, findings)
        .into_iter()
        .map(|c| c.status)
        .collect();
    assert_eq!(
        statuses,
        [
            Some(Status::Met),
            Some(Status::Unmet),
            Some(Status::Untested)
        ]
    );
}

#[test]
fn a_criterion_is_kept_on_one_line_and_cut_to_its_length() {
    let text = format!("{}\n a  b ", "é".repeat(MAX_TEXT - 2));
    let item = json!({"text": text, "kind": "error", "reference": " x\ty "});
    let criteria = criteria_in(&called(json!({ "items": [item] }))).unwrap();
    assert_eq!(
        criteria.items[0].text,
        format!("{} a", "é".repeat(MAX_TEXT - 2))
    );
    assert_eq!(criteria.items[0].reference.as_deref(), Some("x y"));
}

#[test]
fn criteria_round_trip_through_json() {
    let mut criteria = of(&[Kind::ExactOutput], true);
    criteria.items[0].reference = Some("multipart.txt".to_string());
    let json = serde_json::to_value(&criteria).unwrap();
    assert_eq!(json["items"][0]["kind"], "exact_output");
    assert_eq!(serde_json::from_value::<Criteria>(json).unwrap(), criteria);
    let plain = serde_json::to_value(of(&[Kind::Behaviour], false)).unwrap();
    assert!(plain.get("strict").is_none(), "{plain}");
}
