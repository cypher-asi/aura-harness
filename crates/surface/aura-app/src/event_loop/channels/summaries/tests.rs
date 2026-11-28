use super::*;
use aura_model::{ModelRequest, ModelResponse, ReasonerError};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A channel store in `dir`.
fn store_in(dir: &Path) -> ChannelStore {
    ChannelStore::for_store(&aura_store_db::RocksStore::open(dir, false).unwrap())
}

/// Summaries kept in a channel store in `dir.path()/db`, and moved from
/// an old file in `dir`.
fn kept_in(dir: &tempfile::TempDir) -> PostSummaries {
    PostSummaries::new(Some(store_in(&dir.path().join("db"))), Some(dir.path()))
}

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
    let summaries = Arc::new(kept_in(&dir));
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
    {
        let first = kept_in(&dir);
        first.put(key_of(LONG), "Footer links".to_string()).await;
    }
    assert!(!dir.path().join(SUMMARY_FILE).exists(), "no plaintext file");

    let restarted = Arc::new(kept_in(&dir));
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
    let summaries = Arc::new(PostSummaries::new(None, None));
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
    let summaries = Arc::new(PostSummaries::new(None, None));
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
async fn an_old_plaintext_file_is_moved_into_the_store_and_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(SUMMARY_FILE);
    let old = format!(
        r#"{{"entries":[{{"key":"{}","summary":"Footer links"}},{{"key":"k0","summary":"bad key"}}]}}"#,
        key_of(LONG)
    );
    std::fs::write(&file, old).unwrap();
    std::fs::write(dir.path().join("channel-post-summaries.json.tmp"), "half").unwrap();
    {
        let summaries = kept_in(&dir);
        assert_eq!(
            summaries.get(&key_of(LONG)).await.as_deref(),
            Some("Footer links")
        );
    }
    assert!(!file.exists(), "the plaintext file is gone");
    assert!(!dir.path().join("channel-post-summaries.json.tmp").exists());
    // Read straight from the store, as the next run does.
    let store = store_in(&dir.path().join("db"));
    let moved = store.post_summary(&key_of(LONG)).unwrap();
    assert_eq!(moved.as_deref(), Some("Footer links"));
}

#[tokio::test]
async fn an_unreadable_old_file_is_deleted_and_counts_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(SUMMARY_FILE);
    std::fs::write(&file, "not json").unwrap();
    let summaries = kept_in(&dir);
    assert_eq!(summaries.get(&key_of(LONG)).await, None);
    assert!(!file.exists());
    summaries.put(key_of(LONG), "Footer links".into()).await;
    assert!(
        !file.exists(),
        "summaries are never written to a file again"
    );
}

#[tokio::test]
async fn without_a_store_summaries_stay_in_memory_and_the_old_file_is_left() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(SUMMARY_FILE);
    std::fs::write(&file, r#"{"entries":[]}"#).unwrap();
    let summaries = PostSummaries::new(None, Some(dir.path()));
    summaries.put(key_of(LONG), "Footer links".into()).await;
    assert_eq!(
        summaries.get(&key_of(LONG)).await.as_deref(),
        Some("Footer links")
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), r#"{"entries":[]}"#);
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

#[test]
fn the_coordinators_typed_messages_are_listed_under_their_prompt_key() {
    use aura_ui::events::{ActivityData, MessageData, MessageRole};
    let typed = |text: &str| MessageData::new(MessageRole::User, text);
    let history = vec![
        typed(LONG),
        MessageData::new(MessageRole::Assistant, "Done."),
        MessageData {
            command: Some("cargo test".to_string()),
            ..typed("cargo test")
        },
        MessageData {
            activity: Some(ActivityData::Thinking {
                text: "hm".to_string(),
                duration_ms: None,
            }),
            ..typed("hm")
        },
        typed("RE36: Parser done\n(inbox reply 7 from RE36, Parser)\nAll green."),
        typed("  "),
        typed("ship it"),
    ];
    assert_eq!(
        prompts_of(&history),
        [
            (prompt_key(LONG), LONG.to_string()),
            (prompt_key("ship it"), "ship it".to_string()),
        ]
    );
}

#[tokio::test]
async fn a_typed_message_is_summarized_under_its_prompt_key() {
    let summaries = Arc::new(PostSummaries::new(None, None));
    let stub = Stub::answering(Some("Footer links open in new tabs"));
    let (tx, mut rx) = mpsc::channel(16);
    let typed = vec![(prompt_key(LONG), LONG.to_string())];
    summarize_posts(summaries, tx, "c1".into(), typed, Some(asker(&stub))).await;
    let summary = "Footer links open in new tabs".to_string();
    assert_eq!(
        sent(&mut rx).await,
        [("c1".to_string(), prompt_key(LONG), summary)]
    );
}

#[tokio::test]
async fn memory_holds_a_bounded_share_and_the_store_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let summaries = kept_in(&dir);
    let key = |n: usize| key_of(&format!("message {n}"));
    for n in 0..=MAX_KEPT {
        summaries.put(key(n), format!("s{n}")).await;
    }
    assert!(lock(&summaries.kept).len() <= MAX_KEPT);
    assert_eq!(
        summaries.get(&key(0)).await.as_deref(),
        Some("s0"),
        "read from the store"
    );
}
