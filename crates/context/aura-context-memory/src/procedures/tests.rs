
use super::*;

#[test]
fn sequences_match_length_diff_gt_1() {
    let a: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
    let b: Vec<String> = vec!["a".into()];
    assert!(!sequences_match(&a, &b));
}

#[test]
fn sequences_match_above_threshold() {
    let a: Vec<String> = vec!["read".into(), "write".into(), "build".into(), "test".into()];
    let b: Vec<String> = vec![
        "read".into(),
        "write".into(),
        "build".into(),
        "deploy".into(),
    ];
    assert!(sequences_match(&a, &b));
}

#[test]
fn sequences_match_below_threshold() {
    let a: Vec<String> = vec!["read".into(), "write".into(), "build".into()];
    let b: Vec<String> = vec!["deploy".into(), "test".into(), "lint".into()];
    assert!(!sequences_match(&a, &b));
}

#[test]
fn sequences_match_empty() {
    let a: Vec<String> = vec![];
    let b: Vec<String> = vec![];
    assert!(!sequences_match(&a, &b));
}

#[test]
fn merge_steps_longer_new_preferred() {
    let existing = vec!["a".into(), "b".into()];
    let new = vec!["a".into(), "b".into(), "c".into()];
    assert_eq!(merge_steps(&existing, &new), new);
}

#[test]
fn merge_steps_shorter_existing_kept() {
    let existing = vec!["a".into(), "b".into(), "c".into()];
    let new = vec!["a".into(), "b".into()];
    assert_eq!(merge_steps(&existing, &new), existing);
}

#[test]
fn derive_name_from_hint() {
    let steps = vec!["a".into()];
    let name = derive_name(&steps, Some("deploy the application now"));
    assert_eq!(name, "deploy the application now");
}

#[test]
fn derive_name_from_steps() {
    let steps = vec!["build".into(), "test".into(), "deploy".into()];
    let name = derive_name(&steps, None);
    assert_eq!(name, "build -> test -> deploy");
}

#[test]
fn derive_name_filters_read() {
    let steps = vec!["read".into(), "build".into(), "deploy".into()];
    let name = derive_name(&steps, None);
    assert_eq!(name, "build -> deploy");
}

#[test]
fn derive_name_all_read_unnamed() {
    let steps = vec!["read_file".into(), "read_dir".into()];
    let name = derive_name(&steps, None);
    assert_eq!(name, "unnamed_procedure");
}

#[test]
fn apply_success_ema_success() {
    let mut rate = 0.5;
    apply_success_ema(&mut rate, true);
    let expected = 0.8f32.mul_add(0.5, 0.2);
    assert!((rate - expected).abs() < 1e-6);
}

#[test]
fn apply_success_ema_failure() {
    let mut rate = 0.5;
    apply_success_ema(&mut rate, false);
    assert!((rate - 0.4).abs() < 1e-6);
}

#[test]
fn tokenize_words_basic() {
    let words = tokenize_words("hello, world! foo ba");
    assert!(words.contains(&"hello"));
    assert!(words.contains(&"world"));
    assert!(words.contains(&"foo"));
    assert!(!words.contains(&"ba"));
}

#[test]
fn tokenize_words_punctuation_stripped() {
    let words = tokenize_words("(deploy) [build]");
    assert!(words.contains(&"deploy"));
    assert!(words.contains(&"build"));
}

#[test]
fn tokenize_words_min_length_3() {
    let words = tokenize_words("a ab abc abcd");
    assert!(!words.contains(&"a"));
    assert!(!words.contains(&"ab"));
    assert!(words.contains(&"abc"));
    assert!(words.contains(&"abcd"));
}

#[test]
fn skill_relevance_high_overlap() {
    let score = compute_skill_relevance(
        "deploy production",
        "deploy the application",
        &["build".into(), "push".into(), "deploy".into()],
        "deploy applications to production environments",
    );
    assert!(score > 0.3, "expected high relevance, got {score}");
}

#[test]
fn skill_relevance_no_overlap() {
    let score = compute_skill_relevance(
        "user preferences",
        "remember dark mode setting",
        &["store_preference".into()],
        "deploy applications to production environments",
    );
    assert!(score < 0.3, "expected low relevance, got {score}");
}

#[test]
fn skill_relevance_empty_description() {
    let score = compute_skill_relevance("deploy flow", "deploy", &["build".into()], "");
    assert!((score - 0.5).abs() < f32::EPSILON);
}
