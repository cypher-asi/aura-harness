use super::*;
use aura_model::MockProvider;

#[test]
fn titles_are_written_by_the_classifier_unless_configured() {
    let haiku = || "claude/claude-haiku-5-5".to_string();
    assert_eq!(title_model_with(haiku(), None), "claude/claude-haiku-5-5");
    assert_eq!(title_model_with(haiku(), Some(" my-model ")), "my-model");
    assert_eq!(
        title_model_with(haiku(), Some("  ")),
        "claude/claude-haiku-5-5"
    );
}

#[test]
fn sanitize_keeps_only_the_title() {
    let sanitize = |reply: &str| sanitize(reply, MAX_TITLE_CHARS);
    assert_eq!(
        sanitize("\n  \"Task build issue.\"\nmore").as_deref(),
        Some("Task build issue")
    );
    assert_eq!(
        sanitize("Title: Agent trace persistence").as_deref(),
        Some("Agent trace persistence")
    );
    assert_eq!(sanitize("**Fix login**").as_deref(), Some("Fix login"));
    assert_eq!(sanitize("  \n \"\" "), None);
    let long = sanitize(&"word ".repeat(30)).unwrap();
    assert!(long.chars().count() <= MAX_TITLE_CHARS);
    assert!(!long.ends_with(' '));
}

#[test]
fn fallback_uses_the_start_of_the_prompt() {
    assert_eq!(
        fallback_title("\n  fix   the build\nthen run tests"),
        "fix the build"
    );
    let long =
        fallback_title("agent name in left side should update after there is info, use Haiku");
    assert_eq!(long, "agent name in left side should update");
    assert_eq!(fallback_title("   "), "New chat");
    assert_eq!(fallback_title(&"é".repeat(60)).chars().count(), 40);
}

#[test]
fn bench_title_names_the_benchmark() {
    assert_eq!(
        bench_title(&BenchSource::Prompt, "fix the build\nthen run tests"),
        "fix the build"
    );
    assert_eq!(
        bench_title(
            &BenchSource::Examples { tasks: Vec::new() },
            "3 example coding tasks"
        ),
        "Bench: 3 example coding tasks"
    );
}

#[test]
fn subtask_names_are_tighter_than_chat_titles() {
    let long = sanitize(&"word ".repeat(30), MAX_SUBTITLE_CHARS).unwrap();
    assert!(long.chars().count() <= MAX_SUBTITLE_CHARS);
    assert_eq!(
        fallback_subtitle("Implement the engine core in the runtime crate"),
        "Engine core"
    );
    assert_eq!(
        fallback_subtitle("finish the INBOX tests\nthen report"),
        "INBOX tests"
    );
    assert_eq!(fallback_subtitle("add retries"), "Retries");
    // Prompts not opening with a filler verb keep their words.
    assert_eq!(
        fallback_subtitle("review the diff\nthen the tests"),
        "review the diff"
    );
    // A lone verb is kept rather than leaving nothing.
    assert_eq!(fallback_subtitle("Implement"), "Implement");
    assert_eq!(fallback_subtitle("  "), "Subtask");
    assert!(fallback_subtitle(&"é".repeat(60)).chars().count() <= MAX_SUBTITLE_FALLBACK_CHARS);
    assert_eq!(bench_subtitle("Fix the parser"), "Parser");
}

#[tokio::test]
async fn subtasks_are_named_with_the_subtask_prompt_and_limit() {
    let provider = MockProvider::simple_response("Engine core");
    let config = AgentLoopConfig::for_agent("aura-claude-opus-4-7");
    let title = generate_kind(
        TitleKind::Subtask,
        &provider,
        &config,
        "aura-claude-haiku-4-5",
        "Implement the engine core",
        &[],
    )
    .await;
    assert_eq!(title, "Engine core");
    assert_eq!(
        TitleKind::Subtask.system_prompt(),
        SUBTASK_TITLE_SYSTEM_PROMPT
    );
    assert!(TitleKind::Subtask.max_chars() < TitleKind::Chat.max_chars());
}

#[test]
fn last_reply_skips_empty_assistant_messages() {
    let messages = vec![
        Message::user("hi"),
        Message::assistant("Fixed the import."),
        Message::user("thanks"),
        Message::assistant(""),
    ];
    assert_eq!(last_reply(&messages), "Fixed the import.");
    assert_eq!(last_reply(&[]), "");
}

#[tokio::test]
async fn generate_asks_the_title_model_and_cleans_its_answer() {
    let provider = MockProvider::simple_response("\"Task build issue.\"");
    let config = AgentLoopConfig::for_agent("aura-claude-opus-4-7");
    let title = generate(
        &provider,
        &config,
        "aura-claude-haiku-4-5",
        "the build fails",
        &[Message::assistant("Fixed it.")],
    )
    .await;
    assert_eq!(title, "Task build issue");
}

/// A provider whose calls never return.
struct Silent;

#[async_trait::async_trait]
impl ModelProvider for Silent {
    fn name(&self) -> &'static str {
        "silent"
    }

    async fn complete(
        &self,
        _request: ModelRequest,
    ) -> Result<aura_model::ModelResponse, aura_model::ReasonerError> {
        std::future::pending().await
    }

    async fn health_check(&self) -> bool {
        true
    }
}

#[tokio::test(start_paused = true)]
async fn a_title_model_that_never_answers_gives_way_to_the_prompt() {
    let config = AgentLoopConfig::for_agent("aura-claude-opus-4-7");
    let started = tokio::time::Instant::now();
    let title = generate(
        &Silent,
        &config,
        "aura-claude-haiku-4-5",
        "fix the build\nthen run tests",
        &[],
    )
    .await;
    assert_eq!(title, "fix the build");
    // The paused clock jumps straight to the deadline, no sooner.
    assert_eq!(started.elapsed(), TITLE_TIMEOUT);
}

#[tokio::test]
async fn thread_subjects_use_the_subject_prompt_and_limit() {
    let provider = MockProvider::simple_response("Subject: \"Inbox subject titles.\"");
    let config = AgentLoopConfig::for_agent("aura-claude-opus-4-7");
    let subject = subject_for(
        &provider,
        &config,
        "aura-claude-haiku-4-5",
        "Work ONLY in /home/n3o/code/x\nUse haiku for inbox subjects",
    )
    .await;
    assert_eq!(subject.as_deref(), Some("Inbox subject titles"));
    assert_eq!(
        TitleKind::Subject.system_prompt(),
        MAIL_SUBJECT_SYSTEM_PROMPT
    );
    assert!(TitleKind::Subject.max_chars() <= TitleKind::Chat.max_chars());
}

#[tokio::test(start_paused = true)]
async fn a_silent_model_writes_no_thread_subject() {
    let config = AgentLoopConfig::for_agent("aura-claude-opus-4-7");
    let subject = subject_for(&Silent, &config, "aura-claude-haiku-4-5", "Review it").await;
    assert_eq!(subject, None, "the thread keeps the subject it has");
}

#[tokio::test]
async fn an_empty_answer_writes_no_thread_subject() {
    let config = AgentLoopConfig::for_agent("aura-claude-opus-4-7");
    let subject = subject_for(
        &MockProvider::simple_response("  \n\"\" "),
        &config,
        "aura-claude-haiku-4-5",
        "Review it",
    )
    .await;
    assert_eq!(subject, None);
}
