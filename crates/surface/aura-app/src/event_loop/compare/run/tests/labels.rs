//! The label of each prompt, asked for once and shared by every run.

use super::super::labels::Labels;
use aura_core_types::{Difficulty, PromptCategory};
use aura_gate::judge::classify::Label;
use std::sync::atomic::{AtomicU32, Ordering};

fn label(difficulty: Difficulty) -> Label {
    Label {
        category: PromptCategory::Debugging,
        difficulty,
        assessment: None,
    }
}

#[tokio::test]
async fn a_prompt_is_classified_once_and_its_label_shared() {
    let labels = Labels::default();
    let calls = AtomicU32::new(0);
    let classify = |difficulty| {
        calls.fetch_add(1, Ordering::SeqCst);
        async move { Ok(label(difficulty)) }
    };
    let (a, b) = tokio::join!(
        labels.of("fix it", || classify(Difficulty::Hard)),
        labels.of("fix it", || classify(Difficulty::Easy)),
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(a.unwrap().difficulty, Difficulty::Hard);
    assert_eq!(
        b.unwrap().difficulty,
        Difficulty::Hard,
        "the first label wins"
    );
    let other = labels.of("explain it", || classify(Difficulty::Easy)).await;
    assert_eq!(other.unwrap().difficulty, Difficulty::Easy);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "another prompt is asked of"
    );
}

#[tokio::test]
async fn a_failed_label_is_asked_for_again() {
    let labels = Labels::default();
    let failed = labels
        .of("fix it", || async { Err("timed out".to_string()) })
        .await;
    assert_eq!(failed, Err("timed out".to_string()));
    let again = labels
        .of("fix it", || async { Ok(label(Difficulty::Medium)) })
        .await;
    assert_eq!(again.unwrap().difficulty, Difficulty::Medium);
}
