
use super::*;

#[test]
fn model_name_rejects_empty() {
    assert!(ModelName::try_new("").is_err());
    assert!(ModelName::try_new("   ").is_err());
}

#[test]
fn model_name_accepts_valid() {
    let name = ModelName::try_new("claude-opus-4-7").unwrap();
    assert_eq!(name.as_str(), "claude-opus-4-7");
    assert_eq!(format!("{name}"), "claude-opus-4-7");
}

#[test]
fn max_tokens_rejects_zero() {
    assert!(MaxTokens::try_new(0).is_err());
}

#[test]
fn max_tokens_round_trips() {
    let v = MaxTokens::try_new(16_384).unwrap();
    assert_eq!(v.get(), 16_384);
    let raw: u32 = v.into();
    assert_eq!(raw, 16_384);
}

#[test]
fn temperature_rejects_out_of_range() {
    assert!(Temperature::try_new(-0.1).is_err());
    assert!(Temperature::try_new(2.1).is_err());
    assert!(Temperature::try_new(f32::NAN).is_err());
    assert!(Temperature::try_new(f32::INFINITY).is_err());
}

#[test]
fn temperature_accepts_bounds() {
    assert!(Temperature::try_new(0.0).is_ok());
    assert!(Temperature::try_new(2.0).is_ok());
    assert_eq!(Temperature::default(), Temperature::try_new(1.0).unwrap());
}

#[test]
fn replies_get_the_models_output_limit_and_never_more() {
    let limit = crate::model_catalog::max_output_tokens("aura-claude-haiku-4-5");
    let unset = ModelRequest::builder("aura-claude-haiku-4-5", "sys")
        .try_build()
        .unwrap();
    assert_eq!(unset.max_tokens.get(), limit);
    let over = ModelRequest::builder("aura-claude-haiku-4-5", "sys")
        .max_tokens(limit + 1)
        .try_build()
        .unwrap();
    assert_eq!(over.max_tokens.get(), limit);
    let under = ModelRequest::builder("aura-claude-haiku-4-5", "sys")
        .max_tokens(100)
        .try_build()
        .unwrap();
    assert_eq!(under.max_tokens.get(), 100);
}

#[test]
fn try_build_reports_invalid_inputs() {
    let err = ModelRequest::builder("", "sys").try_build().unwrap_err();
    assert!(matches!(err, ReasonerError::Internal(_)));

    let err = ModelRequest::builder("model", "sys")
        .max_tokens(0)
        .try_build()
        .unwrap_err();
    assert!(matches!(err, ReasonerError::Internal(_)));

    let err = ModelRequest::builder("model", "sys")
        .temperature(3.0)
        .try_build()
        .unwrap_err();
    assert!(matches!(err, ReasonerError::Internal(_)));
}
