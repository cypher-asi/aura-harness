use super::*;
use crate::ids::Hash;

#[test]
fn prompt_payload_round_trips_with_string_forms() {
    let payload = PromptPayload {
        text: "fix the failing test".to_string(),
        category: Some(PromptCategory::Debugging),
        classifier: "claude/claude-haiku-4-5".to_string(),
        route: Route::turn("claude/claude-sonnet-4-6", RouteReason::Default),
        kind: None,
        led: Some(true),
        difficulty: Some(Difficulty::Hard),
    };
    let bytes = payload.to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["category"], "debugging");
    assert_eq!(json["route"]["level"], "turn");
    assert_eq!(json["route"]["reason"], "default");
    assert_eq!(json["route"]["propensity"], 1.0);
    assert!(json.get("kind").is_none(), "a turn names no kind");
    assert_eq!(json["led"], true);
    assert_eq!(json["difficulty"], "hard");
    assert_eq!(PromptPayload::from_bytes(&bytes).unwrap(), payload);
    let older = br#"{"text":"fix it","category":"debugging","classifier":"","route":{"level":"turn","model":"m","reason":"default","explored":false,"propensity":1.0}}"#;
    let older = PromptPayload::from_bytes(older).unwrap();
    assert_eq!((older.led, older.difficulty), (None, None));
    let unrouted = String::from_utf8(PromptPayload::new("hi").to_bytes()).unwrap();
    assert!(
        !unrouted.contains("difficulty") && !unrouted.contains("category"),
        "a prompt Gate does not route is stored without a label: {unrouted}"
    );
    let unclassified = br#"{"text":"hi","category":"unclassified","classifier":"","route":{"level":"turn","model":"m","reason":"pinned","explored":false,"propensity":1.0}}"#;
    assert_eq!(
        PromptPayload::from_bytes(unclassified).unwrap().category,
        None,
        "older records stored unclassified prompts by name"
    );
    let unknown = br#"{"text":"hi","category":"bugfix","classifier":"","route":{"level":"turn","model":"m","reason":"pinned","explored":false,"propensity":1.0}}"#;
    assert!(PromptPayload::from_bytes(unknown).is_err());
}

#[test]
fn difficulties_round_trip_by_name() {
    for difficulty in Difficulty::ALL {
        assert_eq!(Difficulty::from_name(difficulty.as_str()), Some(difficulty));
    }
    assert_eq!(Difficulty::from_name("tricky"), None);
}

#[test]
fn subtask_prompt_payload_records_its_kind_and_level() {
    let payload = PromptPayload {
        category: Some(PromptCategory::Research),
        route: Route::subtask("claude/claude-haiku-4-5", RouteReason::Proven),
        kind: Some("explorer".to_string()),
        ..PromptPayload::new("find the parser")
    };
    let bytes = payload.to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["route"]["level"], "subtask");
    assert_eq!(json["kind"], "explorer");
    assert_eq!(PromptPayload::from_bytes(&bytes).unwrap(), payload);
}

#[test]
fn raw_text_is_not_a_prompt_payload() {
    assert!(PromptPayload::from_bytes(b"hello").is_err());
}

#[test]
fn outcome_payload_round_trips() {
    let outcome = OutcomePayload {
        prompt_tx: Hash::from_content(b"prompt"),
        category: PromptCategory::SmallChange,
        model: "aura-claude-sonnet-4-6".to_string(),
        success: Some(false),
        signal: OutcomeSignal::ChecksFailed,
        cost_usd: 0.042,
        subtask_cost_usd: 0.0,
        latency_ms: 12_345,
    };
    let bytes = outcome.to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["signal"], "checks_failed");
    assert_eq!(json["prompt_tx"], outcome.prompt_tx.to_hex());
    assert_eq!(OutcomePayload::from_bytes(&bytes).unwrap(), outcome);
}

#[test]
fn signals_say_success_failure_or_nothing() {
    assert_eq!(OutcomeSignal::ChecksPassed.success(), Some(true));
    assert_eq!(OutcomeSignal::JudgedPass.success(), Some(true));
    assert!(OutcomeSignal::JudgedPass.is_turn_end());
    assert_eq!(OutcomeSignal::JudgedExcused.success(), None);
    assert!(OutcomeSignal::JudgedExcused.voids());
    assert!(!OutcomeSignal::JudgedFail.voids());
    assert_eq!(OutcomeSignal::Unavailable.success(), None);
    assert!(OutcomeSignal::Unavailable.voids() && OutcomeSignal::Unavailable.is_turn_end());
    assert!(!OutcomeSignal::JudgedExcused.is_turn_end());
    for failure in [
        OutcomeSignal::Error,
        OutcomeSignal::ChecksFailed,
        OutcomeSignal::Rewind,
        OutcomeSignal::RetryStronger,
        OutcomeSignal::Redone,
        OutcomeSignal::JudgedFail,
    ] {
        assert_eq!(failure.success(), Some(false), "{failure}");
    }
    assert_eq!(OutcomeSignal::Cancelled.success(), None);
    assert!(OutcomeSignal::Cancelled.is_turn_end() && !OutcomeSignal::Cancelled.voids());
    assert!(!OutcomeSignal::Redone.is_turn_end());
    assert!(
        serde_json::from_value::<OutcomeSignal>(serde_json::json!("none")).is_err(),
        "every outcome says how the work went"
    );
    assert_eq!(
        serde_json::to_value(OutcomeSignal::Redone).unwrap(),
        OutcomeSignal::Redone.as_str()
    );
}

#[test]
fn with_signal_keeps_the_work_and_rejudges_it() {
    let outcome = OutcomePayload {
        prompt_tx: Hash::from_content(b"p"),
        category: PromptCategory::Question,
        model: "m".to_string(),
        success: None,
        signal: OutcomeSignal::Cancelled,
        cost_usd: 0.5,
        subtask_cost_usd: 0.0,
        latency_ms: 10,
    };
    let passed = outcome.with_signal(OutcomeSignal::JudgedPass);
    assert_eq!(passed.success, Some(true));
    assert_eq!(passed.signal, OutcomeSignal::JudgedPass);
    assert_eq!((passed.cost_usd, passed.latency_ms), (0.5, 10));
}

#[test]
fn category_names_parse_back() {
    for category in PromptCategory::CLASSES {
        assert_eq!(PromptCategory::from_name(category.as_str()), Some(category));
        let json = serde_json::to_value(category).unwrap();
        assert_eq!(json, category.as_str());
    }
    assert_eq!(PromptCategory::from_name("bugfix"), None);
    assert_eq!(PromptCategory::from_name("unclassified"), None);
}

#[test]
fn level_and_reason_names_match_their_stored_form() {
    for level in [RouteLevel::Turn, RouteLevel::Subtask] {
        assert_eq!(serde_json::to_value(level).unwrap(), level.as_str());
    }
    for reason in [
        RouteReason::Pinned,
        RouteReason::Default,
        RouteReason::Proven,
        RouteReason::Explore,
        RouteReason::Compare,
        RouteReason::Escalate,
    ] {
        assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str());
    }
}
