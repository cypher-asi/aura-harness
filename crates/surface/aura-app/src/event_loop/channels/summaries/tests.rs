use super::*;
use aura_model::{ModelRequest, ModelResponse, ReasonerError};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const LONG: &str = "Please make every link in the footer open in a new tab, and while you are \
                    there check the privacy link points at the new policy page";

/// Answers every call with `reply` (or fails when `None`), counting them,
/// after `delay`.
struct Stub {
    reply: Option<&'static str>,
    calls: AtomicUsize,
    delay: Duration,
}

impl Stub {
    fn answering(reply: Option<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            reply,
            calls: AtomicUsize::new(0),
            delay: Duration::ZERO,
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl ModelProvider for Stub {
    fn name(&self) -> &'static str {
        "stub"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        match self.reply {
            Some(reply) => {
                aura_model::MockProvider::simple_response(reply)
                    .complete(request)
                    .await
            }
            None => Err(ReasonerError::Internal("no model".to_string())),
        }
    }

    async fn health_check(&self) -> bool {
        true
    }
}

fn asker(stub: &Arc<Stub>) -> Asker {
    Asker {
        provider: Arc::clone(stub) as Arc<dyn ModelProvider + Send + Sync>,
        config: AgentLoopConfig::for_agent("aura-claude-opus-4-7"),
        model: "claude/claude-haiku-4-5".to_string(),
    }
}

/// Every summary sent to the UI, in order, once the calls settled.
async fn sent(rx: &mut mpsc::Receiver<UiCommand>) -> Vec<(String, u64, String)> {
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut out = Vec::new();
    while let Ok(command) = rx.try_recv() {
        if let UiCommand::SetPostSummaries {
            channel_id,
            summaries,
        } = command
        {
            out.extend(
                summaries
                    .into_iter()
                    .map(|(seq, s)| (channel_id.clone(), seq, s)),
            );
        }
    }
    out
}

#[tokio::test]
async fn a_long_post_is_summarized_once_and_a_short_one_never() {
    let dir = tempfile::tempdir().unwrap();
    let summaries = Arc::new(PostSummaries::new(Some(dir.path())));
    let stub = Stub::answering(Some("Footer links open in new tabs"));
    let (tx, mut rx) = mpsc::channel(16);
    let posts = vec![(1, "Thanks!".to_string()), (2, LONG.to_string())];
    summarize_posts(
        Arc::clone(&summaries),
        tx.clone(),
        "c1".into(),
        posts.clone(),
        Some(asker(&stub)),
    )
    .await;
    assert_eq!(
        sent(&mut rx).await,
        [(
            "c1".to_string(),
            2,
            "Footer links open in new tabs".to_string()
        )]
    );
    assert_eq!(stub.calls(), 1);

    // Seen again, it comes from what was kept, without a call.
    summarize_posts(
        Arc::clone(&summaries),
        tx,
        "c1".into(),
        posts,
        Some(asker(&stub)),
    )
    .await;
    assert_eq!(sent(&mut rx).await.len(), 1);
    assert_eq!(stub.calls(), 1, "never asked twice");
}

#[tokio::test]
async fn kept_summaries_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let first = PostSummaries::new(Some(dir.path()));
    first.put(key_of(LONG), "Footer links".to_string()).await;
    assert!(dir.path().join(SUMMARY_FILE).exists());

    let restarted = Arc::new(PostSummaries::new(Some(dir.path())));
    let (tx, mut rx) = mpsc::channel(16);
    // No way to ask: only what was kept shows.
    summarize_posts(
        restarted,
        tx,
        "c9".into(),
        vec![(7, LONG.to_string())],
        None,
    )
    .await;
    assert_eq!(
        sent(&mut rx).await,
        [("c9".to_string(), 7, "Footer links".to_string())]
    );
}

#[tokio::test]
async fn a_failed_call_sends_nothing_and_can_be_tried_again() {
    let summaries = Arc::new(PostSummaries::new(None));
    let failing = Stub::answering(None);
    let (tx, mut rx) = mpsc::channel(16);
    let posts = vec![(3, LONG.to_string())];
    summarize_posts(
        Arc::clone(&summaries),
        tx.clone(),
        "c1".into(),
        posts.clone(),
        Some(asker(&failing)),
    )
    .await;
    assert!(sent(&mut rx).await.is_empty(), "the UI keeps its cut text");
    let working = Stub::answering(Some("Footer links"));
    summarize_posts(summaries, tx, "c1".into(), posts, Some(asker(&working))).await;
    assert_eq!(sent(&mut rx).await.len(), 1);
}

#[tokio::test]
async fn a_text_being_summarized_is_not_asked_for_again() {
    let summaries = Arc::new(PostSummaries::new(None));
    let slow = Arc::new(Stub {
        reply: Some("Footer links"),
        calls: AtomicUsize::new(0),
        delay: Duration::from_millis(30),
    });
    let (tx, mut rx) = mpsc::channel(16);
    for seq in [4, 5] {
        let posts = vec![(seq, LONG.to_string())];
        summarize_posts(
            Arc::clone(&summaries),
            tx.clone(),
            "c1".into(),
            posts,
            Some(asker(&slow)),
        )
        .await;
    }
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(slow.calls(), 1);
    assert_eq!(sent(&mut rx).await.len(), 1);
}

#[tokio::test]
async fn the_file_keeps_at_most_its_cap_and_an_unreadable_one_counts_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(SUMMARY_FILE), "not json").unwrap();
    let summaries = PostSummaries::new(Some(dir.path()));
    assert_eq!(summaries.get(&key_of(LONG)).await, None);
    for n in 0..=MAX_SUMMARIES {
        summaries.put(format!("k{n}"), format!("s{n}")).await;
    }
    assert_eq!(summaries.get("k0").await, None, "the oldest went first");
    assert_eq!(summaries.get("k1").await.as_deref(), Some("s1"));
    let kept = read(&dir.path().join(SUMMARY_FILE));
    assert_eq!(kept.len(), MAX_SUMMARIES);
}

#[test]
fn summaries_are_asked_of_haiku_when_the_user_reaches_it() {
    let routes = |ids: &[&str]| ids.iter().map(|id| (*id).to_string()).collect::<Vec<_>>();
    let fallback = || "local/qwen3:4b".to_string();
    assert_eq!(
        summary_model(
            &routes(&["local/qwen3:4b", "claude/claude-haiku-4-5"]),
            fallback()
        ),
        "claude/claude-haiku-4-5"
    );
    assert_eq!(
        summary_model(&routes(&["aura-gpt-5-4"]), fallback()),
        "local/qwen3:4b"
    );
    assert_eq!(key_of(" same text "), key_of("same text"));
}
