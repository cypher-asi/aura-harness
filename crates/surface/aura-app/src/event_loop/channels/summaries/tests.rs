use super::cache::{evict_oldest, text_key, Entry, CACHE_FILE};
use super::*;
use aura_model::MockProvider;
use std::collections::HashMap;
use std::path::Path;

/// A job keeping its summaries in `dir`, and the end its answers come out of.
fn job_in(dir: &Path) -> (SummaryJob, mpsc::Receiver<UiCommand>) {
    let (commands, answers) = mpsc::channel(64);
    let job = SummaryJob {
        path: dir.join(CACHE_FILE),
        model: "aura-claude-haiku-4-5".to_string(),
        config: AgentLoopConfig::for_agent("aura-claude-opus-4-7"),
        commands,
    };
    (job, answers)
}

fn ask(key: &str, text: &str) -> MessageSummaryRequest {
    MessageSummaryRequest {
        key: key.to_string(),
        text: text.to_string(),
    }
}

/// Every answer sent so far, as (key, summary).
fn answers(rx: &mut mpsc::Receiver<UiCommand>) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    while let Ok(command) = rx.try_recv() {
        if let UiCommand::SetMessageSummary(view) = command {
            out.push((view.key, view.summary));
        }
    }
    out
}

const LONG: &str = "Please add a history tab to the right panel of channels that lists \
every message I sent, summarized when long";

#[tokio::test]
async fn a_miss_calls_once_and_the_summary_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (job, mut rx) = job_in(dir.path());
    let provider = MockProvider::simple_response("Add a channel history tab.");
    let cache = SummaryCache::default();
    summarize_all(&cache, Some(&provider), &job, vec![ask("k1", LONG)]).await;
    assert_eq!(provider.call_count(), 1);
    assert_eq!(
        answers(&mut rx),
        [(
            "k1".to_string(),
            Some("Add a channel history tab".to_string())
        )]
    );
    assert!(job.path.exists(), "the summary is written to disk");

    // A new run reads it back and makes no call.
    let restarted = SummaryCache::default();
    let fresh = MockProvider::simple_response("unused");
    summarize_all(&restarted, Some(&fresh), &job, vec![ask("k2", LONG)]).await;
    assert_eq!(fresh.call_count(), 0, "a cache hit makes no call");
    assert_eq!(
        answers(&mut rx),
        [(
            "k2".to_string(),
            Some("Add a channel history tab".to_string())
        )]
    );
}

#[tokio::test]
async fn a_failed_call_answers_none_and_is_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (job, mut rx) = job_in(dir.path());
    let provider = MockProvider::new().with_failure();
    let cache = SummaryCache::default();
    summarize_all(&cache, Some(&provider), &job, vec![ask("k", LONG)]).await;
    assert_eq!(answers(&mut rx), [("k".to_string(), None)]);
    assert_eq!(cache.len(), 0);
    assert!(!job.path.exists(), "nothing new to save");

    // Asked again, it tries again.
    let working = MockProvider::simple_response("Add a tab");
    summarize_all(&cache, Some(&working), &job, vec![ask("k", LONG)]).await;
    assert_eq!(working.call_count(), 1);
    assert_eq!(
        answers(&mut rx),
        [("k".to_string(), Some("Add a tab".into()))]
    );
}

#[tokio::test]
async fn no_kernel_answers_none() {
    let dir = tempfile::tempdir().unwrap();
    let (job, mut rx) = job_in(dir.path());
    summarize_all(&SummaryCache::default(), None, &job, vec![ask("k", LONG)]).await;
    assert_eq!(answers(&mut rx), [("k".to_string(), None)]);
}

#[tokio::test(start_paused = true)]
async fn a_stalled_model_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let (job, mut rx) = job_in(dir.path());
    let latency = SUMMARY_TIMEOUT.as_secs() * 1_000 + 5_000;
    let provider = MockProvider::simple_response("too late").with_latency(latency);
    let cache = SummaryCache::default();
    summarize_all(&cache, Some(&provider), &job, vec![ask("k", LONG)]).await;
    assert_eq!(answers(&mut rx), [("k".to_string(), None)]);
    assert_eq!(cache.len(), 0);
}

#[tokio::test]
async fn an_unreadable_file_is_an_empty_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (job, mut rx) = job_in(dir.path());
    std::fs::write(&job.path, b"not json").unwrap();
    let provider = MockProvider::simple_response("Add a tab");
    summarize_all(
        &SummaryCache::default(),
        Some(&provider),
        &job,
        vec![ask("k", LONG)],
    )
    .await;
    assert_eq!(provider.call_count(), 1);
    assert_eq!(answers(&mut rx).len(), 1);
    let saved = std::fs::read_to_string(&job.path).unwrap();
    assert!(saved.contains(&text_key(LONG)), "the file is replaced");
}

#[test]
fn eviction_keeps_the_newest() {
    let mut entries: HashMap<String, Entry> = (0..10u64)
        .map(|i| {
            let entry = Entry {
                summary: format!("s{i}"),
                at_ms: i,
            };
            (format!("k{i}"), entry)
        })
        .collect();
    evict_oldest(&mut entries, 3);
    let mut kept: Vec<_> = entries.into_keys().collect();
    kept.sort();
    assert_eq!(kept, ["k7", "k8", "k9"]);
}

#[test]
fn haiku_writes_the_summaries_when_the_user_can_run_it() {
    let candidates = vec![
        "claude/claude-opus-4-7".to_string(),
        "claude/claude-haiku-4-5".to_string(),
    ];
    assert_eq!(
        summary_model(&candidates, "local".to_string()),
        "claude/claude-haiku-4-5"
    );
    let none = vec!["claude/claude-opus-4-7".to_string()];
    let fallback = summary_model(&none, "classifier".to_string());
    assert!(!fallback.is_empty());
}

#[test]
fn the_request_is_short_and_carries_the_message() {
    let config = AgentLoopConfig::for_agent("aura-claude-opus-4-7");
    let request = summary_request("aura-claude-haiku-4-5", &config, LONG).unwrap();
    assert_eq!(u32::from(request.max_tokens), MAX_SUMMARY_TOKENS);
    assert_eq!(request.system, MESSAGE_SUMMARY_SYSTEM_PROMPT);
    let asked = request.messages[0].text_content();
    assert!(asked.contains("history tab"), "{asked}");
}
