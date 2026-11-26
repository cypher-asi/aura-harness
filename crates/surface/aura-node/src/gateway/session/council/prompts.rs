//! Prompt builders for council member, synthesis and Second Opinion turns.

use aura_core_protocol::CouncilMechanism;
use aura_core_types::SubagentExit;

use super::dispatch::MemberAnswer;

pub(super) fn build_reference_advisor_prompt(query: &str) -> String {
    format!(
        "\
## Second Opinion reference task\n\n\
You are reviewing the user's request for the final Aura model. Give private advice only.\n\n\
## User request\n\n\
{}\n\n\
## Factual rubric\n\n\
- Ground claims in the request above.\n\
- Mark assumptions explicitly.\n\
- Flag missing evidence and likely risks.\n\
- Recommend concrete next steps for the final model.\n\
- Do not claim to have used tools or inspected external state.",
        query.trim()
    )
}

/// Build the final combine turn: embed the user's question and each
/// member's answer (or failure), then direct `members[0]` to combine
/// them according to the chosen [`CouncilMechanism`].
pub(super) fn build_synthesis_prompt(
    query: &str,
    answers: &[MemberAnswer],
    mechanism: CouncilMechanism,
) -> String {
    let n = answers.len();
    let mut prompt = String::new();
    prompt.push_str(&format!(
        "You are the AURA Council synthesizer. {n} council member model(s) independently answered \
         the user's question. {}\n\n",
        mechanism_intro(mechanism)
    ));

    prompt.push_str("## User question\n\n");
    prompt.push_str(query.trim());

    prompt.push_str("\n\n## Council member answers\n");
    for answer in answers {
        let label = if answer.model_label.is_empty() {
            "(default model)".to_string()
        } else {
            answer.model_label.clone()
        };
        prompt.push_str(&format!("\n### Member {} — `{label}`\n\n", answer.index));
        match &answer.outcome {
            Ok(result) => match &result.exit {
                SubagentExit::Completed => {
                    let body = result.final_message.trim();
                    if body.is_empty() {
                        prompt.push_str("(member returned an empty answer)\n");
                    } else {
                        prompt.push_str(body);
                        prompt.push('\n');
                    }
                }
                SubagentExit::Failed { reason } => {
                    prompt.push_str(&format!("(member failed: {reason})\n"));
                }
                SubagentExit::Cancelled => prompt.push_str("(member was cancelled)\n"),
                SubagentExit::Timeout => prompt.push_str("(member timed out)\n"),
                SubagentExit::Rejected { reason } => {
                    prompt.push_str(&format!("(member rejected: {reason})\n"));
                }
            },
            Err(err) => {
                prompt.push_str(&format!("(member dispatch error: {err})\n"));
            }
        }
    }

    prompt.push_str(mechanism_instruction(mechanism));
    prompt
}

/// Build the final Second Opinion turn. Reference outputs are private
/// guidance, not authoritative facts. Unlike the ordinary Council
/// synthesizer prompt, this does not forbid tool use; the final model
/// remains the acting agent and can verify facts when the request calls
/// for it.
pub(super) fn build_second_opinion_prompt(query: &str, answers: &[MemberAnswer]) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You are the final model in Aura Second Opinion. Reference advisor model(s) reviewed the \
         same user request and produced private guidance below.\n\n\
         Treat reference guidance as advisory, not as ground truth. Your answer must be based on \
         facts present in the conversation, tool results you actually inspect, or explicit user \
         input. If references conflict or make unsupported claims, resolve the conflict using \
         evidence or say what is uncertain. You may call tools if verification or task completion \
         requires it; otherwise answer directly.\n\n",
    );

    prompt.push_str("## User request\n\n");
    prompt.push_str(query.trim());

    prompt.push_str("\n\n## Reference advisor guidance\n");
    for answer in answers {
        let label = if answer.model_label.is_empty() {
            "(default model)".to_string()
        } else {
            answer.model_label.clone()
        };
        prompt.push_str(&format!("\n### Reference {} — `{label}`\n\n", answer.index));
        append_member_outcome(&mut prompt, answer, "reference");
    }

    prompt.push_str(
        "\n## Final response rubric\n\n\
         Write the user-facing answer. Use the references to improve coverage and catch mistakes, \
         but do not cite reference claims as facts unless they are supported by the visible \
         conversation or verified evidence. Be explicit about uncertainty and concrete next steps.",
    );
    prompt
}

pub(super) fn append_member_outcome(prompt: &mut String, answer: &MemberAnswer, label: &str) {
    match &answer.outcome {
        Ok(result) => match &result.exit {
            SubagentExit::Completed => {
                let body = result.final_message.trim();
                if body.is_empty() {
                    prompt.push_str(&format!("({label} returned an empty answer)\n"));
                } else {
                    prompt.push_str(body);
                    prompt.push('\n');
                }
            }
            SubagentExit::Failed { reason } => {
                prompt.push_str(&format!("({label} failed: {reason})\n"));
            }
            SubagentExit::Cancelled => prompt.push_str(&format!("({label} was cancelled)\n")),
            SubagentExit::Timeout => prompt.push_str(&format!("({label} timed out)\n")),
            SubagentExit::Rejected { reason } => {
                prompt.push_str(&format!("({label} rejected: {reason})\n"));
            }
        },
        Err(err) => {
            prompt.push_str(&format!("({label} dispatch error: {err})\n"));
        }
    }
}

/// One-line framing for the chosen mechanism, dropped into the prompt
/// preamble so the synthesizer knows its job before reading the answers.
pub(super) fn mechanism_intro(mechanism: CouncilMechanism) -> &'static str {
    match mechanism {
        CouncilMechanism::Synthesize => "Integrate their answers into ONE combined response.",
        CouncilMechanism::Contrast => {
            "Compare and contrast their answers, surfacing agreements and disagreements."
        }
        CouncilMechanism::SideBySide => {
            "Present each member's answer side by side, faithfully and without merging them."
        }
    }
}

/// The closing instruction block that tells `members[0]` how to combine
/// the members' answers for the chosen mechanism. Every variant forbids
/// tool calls so the combine turn is a single direct text response the
/// UI renders below the council panel.
pub(super) fn mechanism_instruction(mechanism: CouncilMechanism) -> &'static str {
    match mechanism {
        CouncilMechanism::Synthesize => {
            "\n## Synthesize\n\n\
             Write ONE combined answer. Explicitly call out where the members AGREE and where they \
             DISAGREE; when they disagree, weigh the trade-offs and state your single best \
             recommendation. Integrate their answers — do not merely list them. Do NOT call any \
             tools; respond with the synthesized answer directly."
        }
        CouncilMechanism::Contrast => {
            "\n## Contrast\n\n\
             Write a structured comparison of the members' answers. Use a section (or table) for \
             the points where they AGREE and a section for where they DISAGREE, attributing each \
             position to the member(s) that hold it. Highlight the key trade-offs that distinguish \
             the answers. Do NOT collapse everything into a single recommendation — the goal is to \
             make the differences clear so the reader can decide. Do NOT call any tools; respond \
             with the contrast directly."
        }
        CouncilMechanism::SideBySide => {
            "\n## Side by side\n\n\
             Present each member's answer side by side under its own clearly labeled heading \
             (`## Member <index> — <model>`), reproducing each answer faithfully with only light \
             cleanup for readability. Do NOT merge, rank, editorialize, or add a recommendation — \
             keep every member's response distinct and self-contained. Do NOT call any tools; \
             respond with the side-by-side presentation directly."
        }
    }
}
