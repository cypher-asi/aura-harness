//! The model provider the event loop tests' kernels send through.

use super::*;

/// Model whose route refuses the sign-in, for Gate's re-routing.
pub(super) const REFUSING_MODEL: &str = "claude/claude-sonnet-4-6";

/// Answers like a mock model, but holds turn calls whose last message
/// mentions "slow" until the test releases them, and refuses
/// [`REFUSING_MODEL`]. Auxiliary calls, like the chat title, quote the
/// prompt and are answered at once. Gate's judge of failed work blames
/// the model when the work's request says "judge: blame", excuses it
/// when it says "judge: excuse", and gives no verdict otherwise. A turn
/// asked to "reply unverified" ends on the fast tier saying its work
/// isn't verified, and Gate's judge of unverified work blames such a
/// reply when the request says "review: blame", finds it fine when it
/// says "review: fine", and gives no verdict otherwise. Gate's review of
/// work about to stand finds it unfinished when the request says
/// "inspect: blame", finds it unfinished until the model was sent back
/// to it when it says "inspect: once", finds it done when it says
/// "inspect: fine", and gives no verdict otherwise; a model sent back to
/// its work replies that it fixed what the review found. The
/// comparison judge prefers the first attempt, and, given what the user
/// kept, rates it as good as that and the second worse; of work "judged
/// together", it answers once a second judge is asked at the same time,
/// and fails after [`JUDGED_TOGETHER`] without one.
pub(super) struct GatedProvider {
    pub(super) inner: MockProvider,
    pub(super) gate: Arc<Semaphore>,
    pub(super) judges: Arc<Barrier>,
    /// The agent whose kernel sends through this provider
    pub(super) agent: AgentId,
    /// Every request the fixture's kernels sent, in order
    pub(super) sent: SentLog,
}

/// One model request a kernel of the fixture sent, as its provider got
/// it.
#[derive(Clone, Debug)]
pub(super) struct Sent {
    /// The agent whose kernel sent it
    pub(super) agent: AgentId,
    /// Its system prompt, which tells Gate's calls from the work
    pub(super) system: String,
    /// Its prompt-cache scope
    pub(super) cache_salt: Option<String>,
}

/// The requests the fixture's kernels sent, oldest first.
pub(super) type SentLog = Arc<std::sync::Mutex<Vec<Sent>>>;

/// How long the comparison judge of work "judged together" waits for a
/// second judge.
const JUDGED_TOGETHER: Duration = Duration::from_secs(5);

/// What a model sent back to its work by Gate's review replies.
const REPAIRED_REPLY: &str = "Fixed what the review found.";

#[async_trait]
impl ModelProvider for GatedProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        self.sent.lock().unwrap().push(Sent {
            agent: self.agent,
            system: request.system.clone(),
            cache_salt: request.cache_salt.clone(),
        });
        if request.model.as_str() == REFUSING_MODEL {
            return Err(ReasonerError::Api {
                status: 401,
                message: "claude plan models are not available in this session".to_string(),
            });
        }
        let last = request.messages.last().map(|m| m.text_content());
        if request.system == aura_gate::judge::classify::SYSTEM_PROMPT {
            return classify(&request, last.as_deref().unwrap_or_default()).await;
        }
        if let Some(reply) = self.judge_reply(&request.system, last.as_deref()).await {
            return reply?.complete(request).await;
        }
        self.work_reply(request, last.as_deref()).await
    }

    async fn health_check(&self) -> bool {
        true
    }
}

/// The provider that answers one of Gate's judges, `None` when `system`
/// is not a judge's or the judge's case falls through to the work.
type Reply = Option<Result<MockProvider, ReasonerError>>;

impl GatedProvider {
    /// The answer of the judge whose system prompt is `system`, asked
    /// `last`.
    async fn judge_reply(&self, system: &str, last: Option<&str>) -> Reply {
        let asked = last.unwrap_or_default();
        let simple = |reply: &str| Some(Ok(MockProvider::simple_response(reply)));
        if system == aura_gate::judge::criteria::SYSTEM_PROMPT {
            return criteria_reply(asked);
        }
        if system == aura_gate::judge::turn::SYSTEM_PROMPT {
            return simple(turn_reply(asked));
        }
        if system == aura_gate::judge::turn::UNVERIFIED_SYSTEM_PROMPT {
            return simple(unverified_reply(asked));
        }
        if system == aura_gate::judge::turn::REVIEW_SYSTEM_PROMPT {
            return simple(review_reply(asked));
        }
        if system == aura_gate::judge::blind::SYSTEM_PROMPT {
            return Some(self.blind_reply(asked).await);
        }
        None
    }

    /// The blind comparison judge's answer to `asked`.
    async fn blind_reply(&self, asked: &str) -> Result<MockProvider, ReasonerError> {
        if asked.contains("judged together")
            && tokio::time::timeout(JUDGED_TOGETHER, self.judges.wait())
                .await
                .is_err()
        {
            return Err(ReasonerError::Api {
                status: 408,
                message: "no other pair was being judged".to_string(),
            });
        }
        let reply = if asked.contains("## Reference") {
            "A is tidier.\nA_VS_REFERENCE: AS_GOOD\nB_VS_REFERENCE: WORSE\nWINNER: A"
        } else {
            "A is tidier.\nWINNER: A"
        };
        Ok(MockProvider::simple_response(reply))
    }

    /// The answer to a request of the work itself (not a judge's).
    async fn work_reply(
        &self,
        request: ModelRequest,
        last: Option<&str>,
    ) -> Result<ModelResponse, ReasonerError> {
        let runs_check = last.is_some_and(|text| {
            text.contains("run failing check") || text.contains("run passing check")
        });
        if !runs_check
            && request
                .messages
                .iter()
                .any(|m| m.text_content().contains("write the answer"))
        {
            return answer(request, last).await;
        }
        let auxiliary = request.metadata.kind == Some(aura_model::ModelRequestKind::Auxiliary);
        let slow = !auxiliary && last.is_some_and(|text| text.contains("slow"));
        if slow {
            self.gate.acquire().await.unwrap().forget();
        }
        if let Some(response) = scripted(last.unwrap_or_default(), &request) {
            return scripted_reply(request, response).await;
        }
        if let Some(provider) = command_reply(last) {
            return provider.complete(request).await;
        }
        if let Some(reply) = fallback_reply(&request, last, auxiliary) {
            return reply?.complete(request).await;
        }
        self.inner.complete(request).await
    }
}

/// The criteria writer's answer to `asked`: down when told so, else the
/// criteria of the kind asked for, if any.
fn criteria_reply(asked: &str) -> Reply {
    if asked.contains("criteria writer down") {
        return Some(Err(ReasonerError::Api {
            status: 400,
            message: "tool_choice: type \"tool\" and \"any\" are not supported for this model."
                .to_string(),
        }));
    }
    let kind = if asked.contains("(exacting criteria)") {
        "exact_output"
    } else if asked.contains("(plain criteria)") {
        "behaviour"
    } else {
        return None;
    };
    let input = serde_json::json!({
        "items": [{"text": "prints the parse tree", "kind": kind}],
        "strict": kind == "exact_output",
    });
    Some(Ok(MockProvider::new().with_response(
        MockResponse::tool_use(
            "toolu_criteria",
            aura_gate::judge::criteria::CRITERIA_TOOL,
            input,
        ),
    )))
}

/// The turn judge's answer to `asked`.
fn turn_reply(asked: &str) -> &'static str {
    if asked.contains("judge: blame") {
        "The change is wrong.\nVERDICT: MODEL"
    } else if asked.contains("judge: excuse") {
        "The check cannot pass here.\nVERDICT: OTHER"
    } else {
        "Hard to say."
    }
}

/// The judge of unverified replies' answer to `asked`.
fn unverified_reply(asked: &str) -> &'static str {
    let disowned = asked.contains("it isn't verified");
    if disowned && asked.contains("review: blame") {
        "The change is untested and wrong.\nVERDICT: MODEL"
    } else if disowned && asked.contains("review: fine") {
        "The change is right.\nVERDICT: OK"
    } else {
        "Hard to say."
    }
}

/// The reviewer's answer to `asked`.
fn review_reply(asked: &str) -> &'static str {
    let repaired = asked.contains(REPAIRED_REPLY);
    let once = asked.contains("inspect: once");
    if asked.contains("inspect: blame") || (once && !repaired) {
        "1. The fix misses the empty case.\nVERDICT: MODEL"
    } else if once || asked.contains("inspect: fine") {
        "Every requirement is met.\nVERDICT: OK"
    } else {
        "Hard to say."
    }
}

/// `response` to `request`, billed as the model asked for.
async fn scripted_reply(
    request: ModelRequest,
    response: MockResponse,
) -> Result<ModelResponse, ReasonerError> {
    let model = request.model.as_str().to_string();
    let mut response = MockProvider::new()
        .with_response(response)
        .complete(request)
        .await?;
    response.trace.model.clone_from(&model);
    response.model_used = model;
    Ok(response)
}

/// The command a turn whose last message is `last` runs: "run ls",
/// "run passing check" or "run failing check".
fn command_reply(last: Option<&str>) -> Option<MockProvider> {
    let check = |args: &[&str]| {
        MockResponse::tool_use(
            "toolu_check",
            "run_command",
            serde_json::json!({ "program": "cargo", "args": args }),
        )
    };
    let response = if last.is_some_and(|text| text.contains("run ls")) {
        MockResponse::tool_use(
            "toolu_ls",
            "run_command",
            serde_json::json!({ "program": "ls", "args": ["."] }),
        )
    } else if last.is_some_and(|text| text.contains("run passing check")) {
        check(&["check", "--help"])
    } else if last.is_some_and(|text| text.contains("run failing check")) {
        check(&["test", "--no-such-flag"])
    } else {
        return None;
    };
    Some(MockProvider::new().with_response(response))
}

/// The work's answers that are neither scripted nor commands: a failure
/// on "explode", a disowning reply, a repaired reply, and the chat title.
fn fallback_reply(request: &ModelRequest, last: Option<&str>, auxiliary: bool) -> Reply {
    if last.is_some_and(|text| text.contains("explode")) {
        return Some(Err(ReasonerError::Api {
            status: 400,
            message: "the model fell over".to_string(),
        }));
    }
    let disowns = !auxiliary
        && request.model.as_str().contains("haiku")
        && request
            .messages
            .iter()
            .any(|m| m.text_content().contains("reply unverified"));
    if disowns {
        let reply = "I made the change, but it isn't verified.";
        return Some(Ok(MockProvider::simple_response(reply)));
    }
    let sent_back =
        !auxiliary && last.is_some_and(|text| text.contains(aura_gate::followup::REPAIR_OPENING));
    if sent_back {
        return Some(Ok(MockProvider::simple_response(REPAIRED_REPLY)));
    }
    // The chat title is written in the background while the turn runs,
    // racing it on the same model. `inner`'s queue holds the one scripted
    // answer for the turn itself, so an auxiliary call like this must not
    // draw from it.
    auxiliary.then(|| Ok(MockProvider::simple_response("Mock title")))
}

/// The one tool call a turn whose last message is `last` makes, for the
/// delegation tests: "write a note" writes `notes/<model>.txt`, "touch a
/// file" runs `touch touched.txt`, "read the big file" reads `big.txt`,
/// "close every todo" marks todos `a` and `b` completed, "start new
/// todos" replaces the todo list with one item `x`,
/// and "change files by shell" appends the model to `tracked.txt` and
/// `target/out` and writes `shell-<model>.txt` with a shell command, which
/// on a first attempt also deletes `gone.txt`.
fn scripted(last: &str, request: &ModelRequest) -> Option<MockResponse> {
    let call = |name: &str, input: serde_json::Value| {
        Some(MockResponse::tool_use(format!("toolu_{name}"), name, input))
    };
    if last.contains("write a note") {
        let path = format!("notes/{}.txt", request.model.as_str());
        return call(
            "write_file",
            serde_json::json!({ "path": path, "content": "note" }),
        );
    }
    if last.contains("touch a file") {
        return call(
            "run_command",
            serde_json::json!({ "program": "touch", "args": ["touched.txt"] }),
        );
    }
    if last.contains("read the big file") {
        return call("read_file", serde_json::json!({ "path": "big.txt" }));
    }
    if last.contains("close every todo") {
        return call(
            "todo_write",
            serde_json::json!({
                "merge": true,
                "todos": [{ "id": "a", "status": "completed" }, { "id": "b", "status": "completed" }],
            }),
        );
    }
    if last.contains("start new todos") {
        return call(
            "todo_write",
            serde_json::json!({ "todos": [{ "id": "x", "content": "Fresh work" }] }),
        );
    }
    if last.contains("change files by shell") {
        let model = request.model.as_str();
        let mut script = format!(
            "echo {model} >> tracked.txt && echo {model} > shell-{model}.txt \
             && echo {model} >> target/out"
        );
        if !last.contains("A first attempt") {
            script.push_str(" && rm gone.txt");
        }
        return call(
            "run_command",
            serde_json::json!({ "program": "sh", "args": ["-c", script] }),
        );
    }
    None
}

/// A turn told to "write the answer": it writes `answer.txt` naming the
/// model that answered, then says it is done. Calls are billed as the
/// model asked for, so they have a list price.
async fn answer(request: ModelRequest, last: Option<&str>) -> Result<ModelResponse, ReasonerError> {
    let model = request.model.as_str().to_string();
    let provider = if last.is_some_and(|text| text.contains("write the answer")) {
        MockProvider::new().with_response(MockResponse::tool_use(
            "toolu_answer",
            "write_file",
            serde_json::json!({ "path": "answer.txt", "content": model }),
        ))
    } else {
        MockProvider::simple_response(format!("Wrote the answer on {model}."))
    };
    let mut response = provider.complete(request).await?;
    response.trace.model.clone_from(&model);
    response.model_used = model;
    Ok(response)
}

/// What the mock classifier ([`classify`]) finds about a prompt it
/// labels `category`, which adds up to `difficulty`.
pub(super) fn mock_assessment(category: &str, difficulty: Difficulty) -> Assessment {
    let (scope, spec, exploration) = match difficulty {
        Difficulty::Easy => (Scope::OneSpot, Spec::Exact, Exploration::None),
        Difficulty::Medium => (Scope::OneFile, Spec::Clear, Exploration::Some),
        Difficulty::Hard => (Scope::ManyFiles, Spec::Loose, Exploration::Deep),
    };
    Assessment {
        analysis: format!("{category} work"),
        scope,
        spec,
        exploration,
        checkable: false,
        pitfalls: Vec::new(),
    }
}

/// The classifier's answer to `request`, asking about `prompt`: prompts
/// that say "across files" are changes across several files, those that
/// say "plan:" are planning, those that say "fix" are debugging,
/// "classifier down" fails the call, "classifier rambles" gets a reply
/// with no label, and the rest are questions. Prompts that say "(easy)"
/// or "(hard)" get that difficulty; the rest are medium. The answer is a
/// call to the label tool when the request offers it, with an assessment
/// that adds up to that difficulty, else text.
async fn classify(request: &ModelRequest, prompt: &str) -> Result<ModelResponse, ReasonerError> {
    if prompt.contains("classifier down") {
        return Err(ReasonerError::Internal("classifier offline".to_string()));
    }
    if prompt.contains("classifier rambles") {
        return MockProvider::simple_response("I'll start by reading the code.")
            .complete(request.clone())
            .await;
    }
    let label = if prompt.contains("across files") {
        "multi_file_change"
    } else if prompt.contains("plan:") {
        "planning"
    } else if prompt.contains("fix") {
        "debugging"
    } else {
        "question"
    };
    let difficulty = ["easy", "hard"]
        .into_iter()
        .find(|d| prompt.contains(&format!("({d})")))
        .unwrap_or("medium");
    let tool = aura_gate::judge::classify::LABEL_TOOL;
    let answer = if request.tools.iter().any(|t| t.name == tool) {
        let difficulty = Difficulty::from_name(difficulty).unwrap();
        let mut input = serde_json::to_value(mock_assessment(label, difficulty)).unwrap();
        input["category"] = label.into();
        input["pitfalls"] = serde_json::json!([]);
        MockResponse::tool_use("toolu_label", tool, input)
    } else {
        MockResponse::text(format!("{label} {difficulty}"))
    };
    MockProvider::new()
        .with_response(answer)
        .complete(request.clone())
        .await
}
