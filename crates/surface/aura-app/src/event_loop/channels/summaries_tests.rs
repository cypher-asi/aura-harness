use super::*;
use aura_model::MockProvider;
use aura_store_db::RocksStore;
use tempfile::TempDir;

const LONG: &str = "Please go through the parser crate and make every escape sequence round trip, \
then add tests for the odd ones";

fn open(dir: &TempDir) -> ChannelStore {
    let store = RocksStore::open(dir.path(), false).unwrap();
    ChannelStore::for_store(&store)
}

/// Run [`summarize_texts`] for `texts` with `provider`, returning every
/// summary sent to the UI.
async fn run(store: &ChannelStore, texts: &[&str], provider: &MockProvider) -> Vec<MessageSummary> {
    let (tx, mut rx) = mpsc::channel(16);
    let config = AgentLoopConfig::for_agent("aura-claude-opus-4-7");
    let summarizer = Summarizer {
        provider,
        config: &config,
        model: "claude/claude-haiku-4-5",
    };
    let texts = texts.iter().map(ToString::to_string).collect();
    summarize_texts(store, "c1", texts, Some(summarizer), &tx).await;
    drop(tx);
    let mut sent = Vec::new();
    while let Some(command) = rx.recv().await {
        if let UiCommand::MessageSummaries(summaries) = command {
            sent.extend(summaries);
        }
    }
    sent
}

#[tokio::test]
async fn a_message_is_summarized_once_and_the_summary_survives_a_restart() {
    let dir = TempDir::new().unwrap();
    let provider = MockProvider::simple_response("Make parser escapes round trip");
    {
        let store = open(&dir);
        let sent = run(&store, &[LONG], &provider).await;
        assert_eq!(
            sent,
            [MessageSummary {
                text: LONG.to_string(),
                summary: "Make parser escapes round trip".to_string(),
            }]
        );
        assert_eq!(provider.call_count(), 1);
    }
    let store = open(&dir);
    let again = MockProvider::simple_response("Something else");
    let sent = run(&store, &[LONG], &again).await;
    assert_eq!(sent[0].summary, "Make parser escapes round trip");
    assert_eq!(again.call_count(), 0, "served from the cache");
}

#[tokio::test]
async fn a_failed_call_sends_and_caches_nothing() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let failing = MockProvider::simple_response("x").with_failure();
    assert!(run(&store, &[LONG], &failing).await.is_empty());
    assert_eq!(store.summary("c1", &summary_key(LONG)).unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn a_model_that_never_answers_times_out_with_nothing() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let stalled = MockProvider::simple_response("late").with_latency(10 * 60 * 1000);
    assert!(run(&store, &[LONG], &stalled).await.is_empty());
    assert_eq!(store.summary("c1", &summary_key(LONG)).unwrap(), None);
}

#[tokio::test]
async fn without_a_kernel_only_cached_summaries_are_sent() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    store
        .set_summary("c1", &summary_key(LONG), "Cached")
        .unwrap();
    let (tx, mut rx) = mpsc::channel(4);
    let texts = vec![LONG.to_string(), format!("{LONG} again")];
    summarize_texts(&store, "c1", texts, None, &tx).await;
    drop(tx);
    let Some(UiCommand::MessageSummaries(sent)) = rx.recv().await else {
        panic!("no summaries sent");
    };
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].summary, "Cached");
    assert!(rx.recv().await.is_none());
}

#[test]
fn keys_are_hex_hashes_of_the_text() {
    let key = summary_key(LONG);
    assert_eq!(key.len(), 64);
    assert!(key
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    assert_ne!(key, summary_key("other"));
}
