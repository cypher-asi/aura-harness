//! Mid-turn user input: the cancellation-biased queue drain and the
//! append of drained inputs to the conversation history.

use aura_model::Message;

use crate::helpers;
use crate::session::input_queue::InputQueue;
use crate::session::UserInput;

/// Outcome of a single biased-select drain at the top of the turn
/// loop. Separates "queue had inputs to apply" from "external (or
/// in-band) cancellation fired during the drain" so the caller can
/// branch on each path without a flag-passing chain.
pub(super) enum DrainOutcome {
    Drained(Vec<UserInput>),
    Cancelled,
}

/// Atomically drain `queue` with cancellation precedence (Rule 6.3).
///
/// The `select!` is `biased;` so a cancellation that fired before
/// (or alongside) the drain always wins; only a clean drain reaches
/// the caller's message-append step. Atomicity of the message
/// append is preserved because the drain consumes the buffer in one
/// step before any `state.messages` mutation runs — there is no
/// half-write window even if cancellation fires immediately after
/// this returns. When no cancellation token is supplied, the drain
/// runs unconditionally (no select! at all).
pub(super) async fn drain_pending_input(
    queue: &InputQueue,
    cancellation_token: Option<&tokio_util::sync::CancellationToken>,
) -> DrainOutcome {
    match cancellation_token {
        Some(token) => {
            tokio::select! {
                biased;
                () = token.cancelled() => DrainOutcome::Cancelled,
                inputs = queue.drain() => DrainOutcome::Drained(inputs),
            }
        }
        None => DrainOutcome::Drained(queue.drain().await),
    }
}

/// Apply a FIFO-ordered batch of [`UserInput`] entries to the
/// conversation message history.
///
/// - [`UserInput::Message`] entries are appended via
///   [`helpers::append_warning`] so the trailing-user-message merge
///   rule (required for Anthropic `tool_use`/`tool_result` adjacency)
///   is preserved.
/// - [`UserInput::Steer`] entries are wrapped in a
///   `<harness_steer>` envelope so the model can distinguish a
///   user-typed message from a harness-on-behalf directive (the
///   wrapper is unindented free text — no XML escaping needed for
///   the model surface).
/// - [`UserInput::Cancel`] entries are dropped because the
///   cancellation token was already fired by
///   [`InputQueue::push`]; the in-band variant is only enqueued for
///   the tracing paper trail and that paper trail has already been
///   served by the queue itself.
///
/// Exposed to the whole agent loop so the stream pump
/// ([`crate::agent_loop::sampling`]) can call it after a per-`OutputItemDone`
/// drain without duplicating the merge / envelope logic.
pub(in crate::agent_loop) fn apply_user_inputs_to_messages(
    messages: &mut Vec<Message>,
    inputs: Vec<UserInput>,
) {
    for input in inputs {
        match input {
            UserInput::Message(text) => {
                helpers::append_warning(messages, &text);
            }
            UserInput::Steer { instruction } => {
                let body = format!("<harness_steer>\n{instruction}\n</harness_steer>");
                helpers::append_warning(messages, &body);
            }
            UserInput::Cancel => {}
        }
    }
}
