use super::support::payload;
use crate::types::compare::{ComparePayload, ModelTokens};

#[test]
fn unpriced_calls_round_trip_and_older_tokens_load_as_all_priced() {
    let row = ModelTokens {
        model: "glm-9".to_string(),
        calls: 4,
        input_tokens: 1_000,
        output_tokens: 100,
        cost_usd: 0.02,
        unpriced_calls: 3,
        ..ModelTokens::default()
    };
    let json = serde_json::to_value(&row).unwrap();
    assert_eq!(json["unpriced_calls"], 3);
    let back: ModelTokens = serde_json::from_value(json).unwrap();
    assert_eq!(back, row);
    assert!(back.is_incomplete());

    let priced = ModelTokens {
        unpriced_calls: 0,
        ..row.clone()
    };
    let json = serde_json::to_value(&priced).unwrap();
    assert!(json.get("unpriced_calls").is_none(), "0 is not written");

    let older = serde_json::json!({
        "model": "claude-sonnet-4-6",
        "calls": 2,
        "input_tokens": 10,
        "cache_read_tokens": 0,
        "cache_write_tokens": 0,
        "output_tokens": 5,
        "cost_usd": 0.01
    });
    let older: ModelTokens = serde_json::from_value(older).unwrap();
    assert_eq!(older.unpriced_calls, 0);
    assert!(!older.is_incomplete());
}

#[test]
fn a_purpose_round_trips_and_older_tokens_load_as_the_work() {
    let criteria = ModelTokens {
        model: "openai/gpt-6.1-terra".to_string(),
        purpose: Some("criteria".to_string()),
        calls: 3,
        ..ModelTokens::default()
    };
    let json = serde_json::to_value(&criteria).unwrap();
    assert_eq!(json["purpose"], "criteria");
    let back: ModelTokens = serde_json::from_value(json).unwrap();
    assert_eq!(back, criteria);

    let work = ModelTokens {
        purpose: None,
        ..criteria
    };
    let json = serde_json::to_value(&work).unwrap();
    assert!(json.get("purpose").is_none(), "the work's row has none");
    let older = serde_json::json!({
        "model": "claude-sonnet-5-5",
        "calls": 2,
        "input_tokens": 10,
        "cache_read_tokens": 0,
        "cache_write_tokens": 0,
        "output_tokens": 5,
        "cost_usd": 0.01
    });
    let older: ModelTokens = serde_json::from_value(older).unwrap();
    assert_eq!(older.purpose, None);
}

#[test]
fn a_run_counts_the_unpriced_calls_of_its_models() {
    let mut payload = payload();
    let run = &mut payload.runs[0];
    assert_eq!(run.unpriced_calls(), 0);
    run.tokens[0].unpriced_calls = 1;
    run.tokens.push(ModelTokens {
        model: "glm-9".to_string(),
        calls: 2,
        unpriced_calls: 2,
        ..ModelTokens::default()
    });
    run.turns[0].unpriced_calls = 3;
    assert_eq!(run.unpriced_calls(), 3);
    let back = ComparePayload::from_bytes(&payload.to_bytes()).unwrap();
    assert_eq!(back, payload);
}
