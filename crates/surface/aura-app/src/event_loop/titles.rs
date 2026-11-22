//! Chat titles for the agents list.
//!
//! After an agent's first completed turn a small model names what the chat
//! is about ("Task build issue"), shown in place of placeholder names like
//! `Agent 2`. The call goes through the agent's kernel like every model
//! call, after the turn, while the turn's task still owns the session.
//!
//! Subtasks get a subtitle the same way: when a `task` call spawns an
//! instance of a permanent agent, the same model names the subtask from
//! its prompt, shown as `<code>: <subtask>` (`RE36: Fix parser escapes`).

use super::subagents::AgentHub;
use aura_agent::{AgentLoopConfig, KernelModelGateway};
use aura_context_prompts::auxiliary::chat_title::{
    chat_title_user_prompt, subtask_title_user_prompt, CHAT_TITLE_SYSTEM_PROMPT,
    SUBTASK_TITLE_SYSTEM_PROMPT,
};
use aura_core_types::{AgentId, BenchSource};
use aura_model::{
    Message, ModelCallPurpose, ModelProvider, ModelRequest, ModelRequestKind, Role, ThinkingEffort,
    ToolChoice,
};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Environment variable naming the model that writes titles.
const TITLE_MODEL_ENV: &str = "AURA_TITLE_MODEL";
/// How much of the prompt and of the reply the model sees.
const MAX_SIDE_CHARS: usize = 2_000;
/// Longest title kept, in characters.
const MAX_TITLE_CHARS: usize = 50;
/// Longest title made from the prompt when the model gives none.
const MAX_FALLBACK_CHARS: usize = 40;
/// Longest subtask name kept, in characters. Subtask rows sit indented in
/// the tree after the helper's code, so they get less room than chats.
const MAX_SUBTITLE_CHARS: usize = 28;
/// Longest subtask name made from the prompt when the model gives none.
const MAX_SUBTITLE_FALLBACK_CHARS: usize = 24;
/// Leading verbs a subtask name made from its prompt drops: they open most
/// task prompts ("Implement the ...") and say nothing about the subtask.
const FILLER_VERBS: &[&str] = &[
    "implement",
    "add",
    "fix",
    "finish",
    "create",
    "update",
    "write",
    "build",
    "make",
];
/// Articles dropped after a filler verb.
const ARTICLES: &[&str] = &["the", "a", "an"];

/// What a generated title names, which sets its prompt and length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitleKind {
    /// A chat in the agents list.
    Chat,
    /// A subtask a spawned helper works on, shown after its code.
    Subtask,
}

impl TitleKind {
    fn system_prompt(self) -> &'static str {
        match self {
            Self::Chat => CHAT_TITLE_SYSTEM_PROMPT,
            Self::Subtask => SUBTASK_TITLE_SYSTEM_PROMPT,
        }
    }

    fn user_prompt(self, prompt: &str, reply: &str) -> String {
        match self {
            Self::Chat => chat_title_user_prompt(prompt, reply),
            Self::Subtask => subtask_title_user_prompt(prompt),
        }
    }

    fn max_chars(self) -> usize {
        match self {
            Self::Chat => MAX_TITLE_CHARS,
            Self::Subtask => MAX_SUBTITLE_CHARS,
        }
    }

    fn fallback(self, prompt: &str) -> String {
        match self {
            Self::Chat => fallback_title(prompt),
            Self::Subtask => fallback_subtitle(prompt),
        }
    }
}
/// How long the title model gets to answer before the title is made from
/// the prompt instead. A title is cosmetic and its call runs beside the
/// turn, so a stalled provider must not leave the job (and its kernel
/// gateway) waiting for ever; a small model answers well within this.
const TITLE_TIMEOUT: Duration = Duration::from_secs(15);

/// The model that titles chats: `AURA_TITLE_MODEL` when set, else
/// `classifier`, the cheap model Gate classifies prompts with.
pub(super) fn title_model(classifier: String) -> String {
    title_model_with(classifier, std::env::var(TITLE_MODEL_ENV).ok().as_deref())
}

fn title_model_with(classifier: String, configured: Option<&str>) -> String {
    configured
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map_or(classifier, str::to_string)
}

/// Title the chat of `agent` from its first `prompt` in the background,
/// while the turn runs, so the Agents panel shows the name within a second
/// or two instead of after the turn. Only the prompt is used: a turn can
/// take minutes and the prompt already says what the chat is about.
pub(super) fn spawn(
    hub: Arc<AgentHub>,
    agent: AgentId,
    gateway: KernelModelGateway,
    config: AgentLoopConfig,
    model: String,
    prompt: String,
) {
    tokio::spawn(async move {
        let title = generate(&gateway, &config, &model, &prompt, &[]).await;
        hub.apply_title(agent, &title).await;
    });
}

/// Name the subtask `agent` was spawned for from its task `prompt` in the
/// background, the way [`spawn`] titles a chat, so the Agents panel shows
/// `<code>: <subtask>` while the subtask still runs.
pub(super) fn spawn_subtitle(
    hub: Arc<AgentHub>,
    agent: AgentId,
    gateway: KernelModelGateway,
    config: AgentLoopConfig,
    model: String,
    prompt: String,
) {
    tokio::spawn(async move {
        let subtitle =
            generate_kind(TitleKind::Subtask, &gateway, &config, &model, &prompt, &[]).await;
        hub.apply_subtitle(agent, &subtitle).await;
    });
}

/// The subtitle of a subtask spawned under a benchmark's run: the start of
/// its `prompt`, without a model call, as [`bench_title`] names the
/// benchmark, so a run costs only what it measures.
pub(super) fn bench_subtitle(prompt: &str) -> String {
    fallback_subtitle(prompt)
}

/// A title for the chat that opened with `prompt` and got `messages` in
/// reply, written by `model` through `provider`. Falls back to the start
/// of `prompt` when the call fails, says nothing usable or takes longer
/// than [`TITLE_TIMEOUT`].
///
/// Logs never carry the prompt: a title the model wrote is logged, one
/// made from the prompt is not.
pub(super) async fn generate(
    provider: &dyn ModelProvider,
    config: &AgentLoopConfig,
    model: &str,
    prompt: &str,
    messages: &[Message],
) -> String {
    generate_kind(TitleKind::Chat, provider, config, model, prompt, messages).await
}

/// [`generate`] for a title of `kind`: its prompt, its length and its
/// fallback.
async fn generate_kind(
    kind: TitleKind,
    provider: &dyn ModelProvider,
    config: &AgentLoopConfig,
    model: &str,
    prompt: &str,
    messages: &[Message],
) -> String {
    let reply = last_reply(messages);
    let request = ModelRequest::builder(model, kind.system_prompt())
        .messages(vec![Message::user(kind.user_prompt(
            &shorten(prompt, MAX_SIDE_CHARS),
            &shorten(&reply, MAX_SIDE_CHARS),
        ))])
        .tools(Vec::new())
        .tool_choice(ToolChoice::None)
        .thinking_effort(Some(ThinkingEffort::Off))
        .auth_token(config.auth_token.clone())
        .aura_project_id(config.aura_project_id.clone())
        .aura_agent_id(config.aura_agent_id.clone())
        .aura_session_id(config.aura_session_id.clone())
        .aura_org_id(config.aura_org_id.clone())
        .request_kind(ModelRequestKind::Auxiliary)
        .purpose(ModelCallPurpose::Title)
        .try_build();
    let titled = match request {
        Ok(request) => {
            match tokio::time::timeout(TITLE_TIMEOUT, provider.complete(request)).await {
                Ok(Ok(response)) => sanitize(&response.message.text_content(), kind.max_chars()),
                Ok(Err(e)) => {
                    warn!(%model, error = %e, "Could not title the chat");
                    None
                }
                Err(_) => {
                    warn!(
                        %model,
                        timeout_secs = TITLE_TIMEOUT.as_secs(),
                        "The title model did not answer in time; titling from the prompt"
                    );
                    None
                }
            }
        }
        Err(e) => {
            warn!(%model, error = %e, "Could not build the chat title request");
            None
        }
    };
    match titled {
        Some(title) => {
            info!(%model, %title, "Titled chat");
            title
        }
        None => {
            // Made from the prompt, so the title itself is not logged.
            info!(%model, "Titled chat from its prompt");
            kind.fallback(prompt)
        }
    }
}

/// Text of the latest assistant message in `messages`.
fn last_reply(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .filter(|m| m.role == Role::Assistant)
        .map(Message::text_content)
        .find(|text| !text.trim().is_empty())
        .unwrap_or_default()
}

/// The title in the model's `reply`: its first line without quotes, a
/// `Title:` label or trailing punctuation, cut to `max` characters. `None`
/// when nothing is left.
fn sanitize(reply: &str, max: usize) -> Option<String> {
    let line = reply.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line
        .strip_prefix("Title:")
        .or_else(|| line.strip_prefix("title:"))
        .unwrap_or(line);
    let line = line
        .trim()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '“' | '”'))
        .trim_end_matches(['.', '!', '?', ':', ';', ','])
        .trim();
    let title = cut_at_word(line, max);
    (!title.is_empty()).then_some(title)
}

/// The title of an agent that ran a benchmark of `task` from `source`
/// before it got one: what the BENCH tab names the benchmark, without a
/// model call.
pub(super) fn bench_title(source: &BenchSource, task: &str) -> String {
    match source {
        BenchSource::Session { title, .. } => fallback_title(title),
        BenchSource::Prompt => fallback_title(task),
        BenchSource::Examples { .. } => fallback_title(&format!("Bench: {task}")),
    }
}

/// A title made from the start of the user's `prompt`.
fn fallback_title(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = cut_at_word(&collapsed, MAX_FALLBACK_CHARS);
    if title.is_empty() {
        "New chat".to_string()
    } else {
        title
    }
}

/// A subtask name made from the start of its task `prompt`: the first
/// line without a leading filler verb ("Implement the engine core" names
/// "Engine core"), cut shorter than a chat title.
fn fallback_subtitle(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let words: Vec<&str> = line.split_whitespace().collect();
    let is = |word: &str, list: &[&str]| list.iter().any(|w| word.eq_ignore_ascii_case(w));
    let mut start = 0;
    if words.first().is_some_and(|w| is(w, FILLER_VERBS)) {
        start = 1;
        if words.get(1).is_some_and(|w| is(w, ARTICLES)) {
            start = 2;
        }
    }
    // Keep the verb when nothing would be left without it.
    let rest = if start < words.len() {
        &words[start..]
    } else {
        &words[..]
    };
    let mut title = trim_dangling(&cut_at_word(&rest.join(" "), MAX_SUBTITLE_FALLBACK_CHARS));
    if start > 0 && start < words.len() {
        title = capitalize(&title);
    }
    if title.is_empty() {
        "Subtask".to_string()
    } else {
        title
    }
}

/// `text` without the articles, prepositions and conjunctions a cut left
/// dangling at its end ("Engine core in the" reads "Engine core"). Keeps
/// at least the first word.
fn trim_dangling(text: &str) -> String {
    const DANGLING: &[&str] = &[
        "the", "a", "an", "in", "of", "for", "to", "and", "or", "with", "on", "at", "from", "by",
    ];
    let mut words: Vec<&str> = text.split_whitespace().collect();
    while words.len() > 1
        && words
            .last()
            .is_some_and(|w| DANGLING.iter().any(|d| w.eq_ignore_ascii_case(d)))
    {
        words.pop();
    }
    words.join(" ")
}

/// `text` with its first character upper-cased.
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `text` cut to at most `max` characters, at a word boundary when one
/// exists past the first half.
fn cut_at_word(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    let cut = match cut.rfind(' ') {
        Some(space) if space >= max / 2 => &cut[..space],
        _ => cut.as_str(),
    };
    cut.trim_end_matches([' ', ',', ';', ':', '-']).to_string()
}

/// `text` cut to at most `max` characters, marked when cut.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
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
}
