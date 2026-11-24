//! The classifier's call: the category and difficulty of a prompt.

use super::reason_within;
use aura_agent_kernel::Kernel;
use aura_gate_judge::classify::{self, Label};
use std::time::Duration;
use tracing::{info, warn};

/// How long the classifier may take before the turn fails.
const CLASSIFY_TIMEOUT: Duration = Duration::from_secs(20);

/// How many times the classifier is asked when its reply gives no label.
const CLASSIFY_ATTEMPTS: u32 = 2;

/// Label `text` with its category and difficulty by asking `model`
/// through `kernel`, so the call is recorded. A reply without a label is
/// asked for again, up to `CLASSIFY_ATTEMPTS` calls in all.
///
/// # Errors
/// Says why when the request could not be built, a call failed or timed
/// out, or no reply gave both a category and a difficulty: Gate does not
/// route work it could not classify.
pub async fn classify_prompt(
    kernel: &Kernel,
    model: &str,
    text: &str,
    auth_token: Option<String>,
) -> Result<Label, String> {
    let failed = |error: String| {
        warn!(%model, %error, "Could not classify the prompt");
        Err(format!(
            "Gate could not classify the prompt with {model}: {error}"
        ))
    };
    let mut reply = String::new();
    for attempt in 1..=CLASSIFY_ATTEMPTS {
        let request = match classify::request(model, text, auth_token.clone()) {
            Ok(request) => request,
            Err(e) => return failed(format!("could not build the request ({e})")),
        };
        let response = match reason_within(kernel, request, CLASSIFY_TIMEOUT).await {
            Ok(response) => response,
            Err(e) => return failed(e.to_string()),
        };
        if let Some(found) = classify::label_of(&response) {
            let assessment = found.assessment.as_ref();
            info!(
                %model,
                attempt,
                category = %found.category,
                difficulty = %found.difficulty,
                points = assessment.map(classify::points),
                drivers = assessment.map(aura_core_types::Assessment::summary),
                analysis = assessment.map(|a| a.analysis.as_str()),
                "Classified the prompt"
            );
            return Ok(found);
        }
        reply = classify::reply_shown(&response.message);
        warn!(%model, attempt, %reply, "The classifier's reply gave no label");
    }
    failed(format!(
        "its reply gave no category and difficulty ({reply:?})"
    ))
}
