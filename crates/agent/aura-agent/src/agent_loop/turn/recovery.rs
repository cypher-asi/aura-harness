//! Recovery when the model signals stop: the unfinished-action,
//! leaked-tool-markup, no-op and open-todo nudges that re-prompt the
//! model once instead of ending the turn on unfinished work.

use tokio::sync::mpsc::Sender;

use crate::events::AgentLoopEvent;
use crate::session::input_queue::InputQueue;
use crate::session::UserInput;

use super::super::cx::TurnCtx;
use super::super::{decisions, streaming, LoopState};
use super::heuristics::{ends_with_unfinished_action, waits_on_user};
use super::input::apply_user_inputs_to_messages;
use super::{
    TurnLoop, TurnStep, MAX_LEAKED_MARKUP_NUDGES, MAX_NO_OP_TURN_NUDGES, MAX_STALLED_TODO_NUDGES,
};

/// Steering text injected when a turn ends having produced no
/// user-visible output. Kept terse and non-prescriptive: it nudges
/// the model to either act or respond without assuming which the
/// user wanted.
const NO_OP_TURN_NUDGE: &str =
    "Your previous turn ended without sending a response to the user or \
making any changes. If the request needs action, continue and complete it now; otherwise reply to \
the user directly.";

const UNFINISHED_ACTION_NUDGE: &str =
    "You ended your response by promising an immediate action, but ended the turn before \
performing it. Use the available tools to carry out the authorized request now. If you are blocked \
or need a decision, explain that clearly instead of promising work that will not run.";

/// Steering text injected when a turn is about to end because the
/// model emitted tool-call syntax as assistant *text* instead of a
/// native `tool_use` block. The harness scrubbed the markup before it
/// entered history, so no tool executed — this tells the model to
/// re-issue the call through the real tool-calling mechanism.
const LEAKED_TOOL_MARKUP_NUDGE: &str =
    "Your previous message wrote tool-call syntax as plain text, \
so no tool actually ran and the work is unfinished. Re-issue the tool call using the native \
tool-calling mechanism — do not write tool invocations (such as <function_calls>, <invoke>, or \
[tool_use ...]) as text.";

impl TurnLoop {
    /// Decide what follows a sampling request after which the model
    /// signalled stop: keep looping (pending input or a recovery
    /// nudge) or end the turn.
    pub(super) fn after_model_stop(
        &mut self,
        ctx: &TurnCtx<'_>,
        state: &mut LoopState,
        scrubbed_tool_markup: bool,
    ) -> TurnStep {
        // The model signalled stop, but pending user input keeps the
        // turn loop alive — the next iteration's drain will pull the
        // queued context into `state.messages` and feed it to a
        // fresh sampling request.
        if ctx.input_queue.is_some_and(InputQueue::has_pending) {
            return TurnStep::Continue;
        }
        if state.result.output_truncated {
            // Bounded output-limit recovery was exhausted. Do not turn it into
            // another implicit retry through the normal no-op/action guards.
            return TurnStep::Stopped;
        }
        if self.recover_unfinished_action(ctx, state)
            || self.recover_leaked_markup(ctx, state, scrubbed_tool_markup)
            || self.recover_no_op(ctx, state)
            || recover_open_todos(ctx, state)
        {
            return TurnStep::Continue;
        }
        TurnStep::Clean
    }

    /// A visible sentence such as "I'll read the file now" used to bypass
    /// the no-op guard even though nothing continues after EndTurn. Recover
    /// this narrow case once; real answers, blockers and plans still stop.
    fn recover_unfinished_action(&mut self, ctx: &TurnCtx<'_>, state: &mut LoopState) -> bool {
        if !(self.recover_promised_actions
            && !state.task_done_completed
            && ends_with_unfinished_action(&state.messages))
        {
            return false;
        }
        if ctx.run.agent.config.auto_continue_no_op_turns
            && self.unfinished_action_nudges_used < MAX_NO_OP_TURN_NUDGES
        {
            self.unfinished_action_nudges_used += 1;
            let decision = decisions::turn_nudge("unfinished_action", state);
            state.decide(decision);
            emit_no_op_progress(ctx.run.event_tx, "turn_unfinished_action_retry");
            steer(state, UNFINISHED_ACTION_NUDGE.to_string());
            return true;
        }
        emit_no_op_progress(ctx.run.event_tx, "turn_ended_unfinished_action");
        false
    }

    /// Leaked-tool-markup recovery: the model signalled stop, but
    /// the last response had tool-call markup scrubbed from its
    /// text (it wrote a tool call as prose, so nothing executed).
    /// The scrubbed leftover still counts as visible output, so the
    /// no-op nudge would not fire — handle it explicitly here
    /// by re-prompting once to re-issue the call as a native
    /// `tool_use`, instead of ending the turn after a single
    /// message on unfinished work.
    fn recover_leaked_markup(
        &mut self,
        ctx: &TurnCtx<'_>,
        state: &mut LoopState,
        scrubbed_tool_markup: bool,
    ) -> bool {
        if !scrubbed_tool_markup {
            return false;
        }
        if ctx.run.agent.config.auto_continue_no_op_turns
            && self.markup_nudges_used < MAX_LEAKED_MARKUP_NUDGES
        {
            self.markup_nudges_used = self.markup_nudges_used.saturating_add(1);
            let decision = decisions::turn_nudge("leaked_tool_markup", state);
            state.decide(decision);
            emit_no_op_progress(ctx.run.event_tx, "turn_tool_markup_retry");
            steer(state, LEAKED_TOOL_MARKUP_NUDGE.to_string());
            return true;
        }
        emit_no_op_progress(ctx.run.event_tx, "turn_ended_tool_markup");
        false
    }

    /// Never-silent + premature-`EndTurn` recovery: the model
    /// signalled stop with no pending input. If the whole turn
    /// produced nothing the user can see (empty or thinking-only
    /// response), surface a clear progress signal so the client
    /// never reads the stop as a silent hang — and, when enabled
    /// and within budget, inject one nudge and re-sample instead
    /// of ending on a no-op.
    fn recover_no_op(&mut self, ctx: &TurnCtx<'_>, state: &mut LoopState) -> bool {
        if self.turn_had_visible_output || state.had_any_write || state.task_done_completed {
            return false;
        }
        if ctx.run.agent.config.auto_continue_no_op_turns
            && self.no_op_nudges_used < MAX_NO_OP_TURN_NUDGES
        {
            self.no_op_nudges_used = self.no_op_nudges_used.saturating_add(1);
            let decision = decisions::turn_nudge("no_op_turn", state);
            state.decide(decision);
            emit_no_op_progress(ctx.run.event_tx, "turn_no_action_retry");
            steer(state, NO_OP_TURN_NUDGE.to_string());
            return true;
        }
        emit_no_op_progress(ctx.run.event_tx, "turn_ended_no_action");
        false
    }
}

fn recover_open_todos(ctx: &TurnCtx<'_>, state: &mut LoopState) -> bool {
    if !ctx.run.agent.config.continue_open_todos {
        return false;
    }
    let Some(instruction) = open_todo_nudge(state) else {
        return false;
    };
    emit_no_op_progress(ctx.run.event_tx, "turn_open_todos_retry");
    steer(state, instruction);
    true
}

/// Append a harness steer carrying `instruction` to the history.
fn steer(state: &mut LoopState, instruction: String) {
    apply_user_inputs_to_messages(&mut state.messages, vec![UserInput::Steer { instruction }]);
}

/// Steering text that sends the model back to its open todo items, or
/// `None` when the turn should really end: nothing is open, the model
/// is waiting on the user, or recent nudges changed nothing.
pub(super) fn open_todo_nudge(state: &mut LoopState) -> Option<String> {
    use aura_exec_tools::todo::TodoStatus;
    let todos = &state.result.todos;
    let next = todos
        .iter()
        .find(|item| item.status == TodoStatus::InProgress)
        .or_else(|| todos.iter().find(|item| item.status == TodoStatus::Pending))?;
    if waits_on_user(&state.messages) {
        return None;
    }
    if state.todos_at_last_nudge.as_ref() == Some(todos) {
        state.stalled_todo_nudges = state.stalled_todo_nudges.saturating_add(1);
    } else {
        state.stalled_todo_nudges = 0;
    }
    if state.stalled_todo_nudges >= MAX_STALLED_TODO_NUDGES {
        return None;
    }
    let instruction = format!(
        "Your todo list still has open items. Continue with \"{}\" (id: {}). Mark items \
completed with todo_write as you finish them and cancel ones that are no longer needed. If you \
are blocked or need a decision from the user, say so plainly and stop.",
        next.content, next.id
    );
    state.todos_at_last_nudge = Some(todos.clone());
    Some(instruction)
}

/// Emit a `Progress` event describing a turn that ended (or is about
/// to be re-prompted) without making real progress, so the client
/// always shows *why* the stream went quiet instead of appearing to
/// hang. Recognised `stage` values:
///
/// - `"turn_no_action_retry"` — no visible output; about to re-prompt.
/// - `"turn_ended_no_action"` — no visible output; turn terminating.
/// - `"turn_tool_markup_retry"` — model wrote a tool call as text;
///   about to re-prompt to re-issue it natively.
/// - `"turn_ended_tool_markup"` — model wrote a tool call as text and
///   the re-prompt budget is spent; turn terminating.
///
/// The WS sink forwards `Progress` verbatim and the chat client
/// renders unknown stages as their label, so no coordinated client
/// release is needed.
fn emit_no_op_progress(event_tx: Option<&Sender<AgentLoopEvent>>, stage: &str) {
    let message = match stage {
        "turn_unfinished_action_retry" => {
            "Model promised an action and ended early — re-prompting once to perform it."
        }
        "turn_ended_unfinished_action" => {
            "Model ended with unfinished work after the recovery attempt."
        }
        "turn_no_action_retry" => {
            "Model ended its turn without responding or acting — re-prompting once."
        }
        "turn_tool_markup_retry" => {
            "Model wrote a tool call as text, so nothing ran — re-prompting once to retry it."
        }
        "turn_ended_tool_markup" => {
            "Model wrote a tool call as text, so nothing ran, and the retry budget is spent."
        }
        "turn_open_todos_retry" => {
            "Model stopped with todo items still open — steering it to the next one."
        }
        _ => "Model ended its turn without responding or taking any action.",
    };
    streaming::emit(
        event_tx,
        AgentLoopEvent::Progress {
            stage: stage.to_string(),
            tool_name: None,
            elapsed_ms: None,
            message: Some(message.to_string()),
        },
    );
}
