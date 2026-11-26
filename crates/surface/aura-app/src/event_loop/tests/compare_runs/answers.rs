//! What `/compare` answers once [`pinned`]'s comparison ended, and the
//! diffs it shows, pinned byte for byte.

use super::pins::{pinned, Pinned};
use super::*;
use crate::event_loop::compare::view::render;
use aura_ui::events::MessageRole;

/// What `/compare <args>` showed in the first agent's chat: `Ok` for a
/// system line, `Err` for a warning.
async fn answered(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    commands: &mut mpsc::Receiver<UiCommand>,
    args: &str,
) -> Vec<Result<String, String>> {
    compare(state, links, args).await;
    (drain(commands).into_iter())
        .filter_map(|(_, command)| match command {
            UiCommand::ShowMessage(m) if m.role == MessageRole::System => Some(Ok(m.content)),
            UiCommand::ShowWarning(warning) => Some(Err(warning)),
            _ => None,
        })
        .collect()
}

/// What `/compare <args>` answers, in order, once [`Pinned`]'s comparison
/// ended as `payload`, which `/compare` lists as `listed`: the list, its
/// view and runs it does not have, then, once it is dismissed, what its
/// removed copies no longer allow.
fn answers_once_ended(
    payload: &ComparePayload,
    listed: String,
) -> Vec<(&'static str, Result<String, String>)> {
    let removed = "Compare #1's copies were already removed";
    let answers = [
        ("", Ok(listed)),
        ("show", Ok(render("Compare #1", payload))),
        ("show 2", Err("No such comparison; /compare lists them")),
        ("diff c-9", Err("No run c-9 in Compare #1")),
        (
            "apply c-9",
            Err("No run c-9 in Compare #1; runs are named like a-1"),
        ),
        (
            "dismiss",
            Ok("Compare #1: dismissed; removed the copies".to_string()),
        ),
        ("dismiss", Err(removed)),
        ("apply a-1", Err(removed)),
        (
            "diff",
            Err("Compare #1's copies were removed, and their diffs with them"),
        ),
        ("cancel", Ok("No comparison to cancel".to_string())),
        (
            "go",
            Err("Nothing to start: /compare [prompt] prepares a comparison first"),
        ),
    ];
    (answers.into_iter())
        .map(|(args, answer)| (args, answer.map_err(str::to_string)))
        .collect()
}

#[tokio::test]
async fn what_compare_answers_after_a_comparison_is_pinned_byte_for_byte() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let Pinned { payload, .. } = pinned(&fixture, &mut state, &links, &mut commands).await;
    let at = crate::record_loader::format_local_time(payload.started_ms);
    let listed = |status: &str| {
        format!(
            "Comparisons (/compare show <n> for one):\n  1. Compare #1 · {at} · \"write the answer\" · \
             gate without enforced delegation (gate:nodelegate) vs aura-claude-opus-4-7 (cloud) · \
             {status} · A $0.0032 median, 0/1 pass / B $0.0035 median, 1/1 pass"
        )
    };
    for (args, answer) in answers_once_ended(&payload, listed("done")) {
        assert_eq!(
            answered(&mut state, &links, &mut commands, args).await,
            [answer],
            "{args}"
        );
    }
    answered(&mut state, &links, &mut commands, "write the answer").await;
    let waiting = format!(
        "{}\nA comparison is waiting: /compare go starts it.",
        listed("dismissed")
    );
    assert_eq!(
        answered(&mut state, &links, &mut commands, "").await,
        [Ok(waiting)]
    );
    let dropped = answered(&mut state, &links, &mut commands, "cancel").await;
    assert_eq!(dropped, [Ok("Dropped the comparison".to_string())]);
}

#[tokio::test]
async fn compare_diff_shows_the_first_run_of_each_arm_file_by_file() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    pinned(&fixture, &mut state, &links, &mut commands).await;
    compare(&mut state, &links, "diff").await;
    let diffs: Vec<(String, Vec<(String, String)>)> = (drain(&mut commands).into_iter())
        .filter_map(|(_, command)| match command {
            UiCommand::ShowDiff { title, files } => {
                let files = files.into_iter().map(|f| (f.path, f.diff)).collect();
                Some((title, files))
            }
            _ => None,
        })
        .collect();
    let answer = |model: &str| {
        let diff = format!("@@ -0,0 +1 @@\n+{model}\n\\ No newline at end of file\n");
        vec![("answer.txt".to_string(), diff)]
    };
    assert_eq!(
        diffs,
        [
            (format!("Compare #1 a-1 ({SONNET})"), answer(SONNET)),
            (format!("Compare #1 b-1 ({OPUS})"), answer(OPUS)),
        ]
    );
}
