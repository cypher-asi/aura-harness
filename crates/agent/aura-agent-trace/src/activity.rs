//! What a run under way is doing right now, kept from its loop events so
//! a view of the run can say so between the steps it records.

use aura_agent::{AgentLoopEvent, DebugEvent};

/// What a run under way is doing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Doing {
    /// Waiting for the model's reply, none of which has arrived yet
    #[default]
    Waiting,
    /// The model is reasoning
    Thinking,
    /// The model is writing its reply or a tool call
    Writing,
    /// The tools of the last reply are running, by name
    Tools(Vec<String>),
}

/// A run's current activity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Activity {
    pub doing: Doing,
    /// Unix ms the current model call, or the tools of the last reply,
    /// began
    pub since_ms: u64,
    /// Times the current model call started its reply over
    pub restarts: u32,
    /// Why it last did, while the call is still under way
    pub restart_reason: Option<String>,
    /// Model calls the run has finished
    pub calls: u32,
    /// Tools of the last reply still without a result: id and name
    open_tools: Vec<(String, String)>,
}

impl Activity {
    /// A turn began at `at_ms`: its first model call is under way.
    pub fn turn_started(&mut self, at_ms: u64) {
        self.next_call(at_ms);
        self.open_tools.clear();
    }

    /// Fold `event`, seen at `at_ms`, into the activity.
    pub fn on_event(&mut self, event: &AgentLoopEvent, at_ms: u64) {
        match event {
            AgentLoopEvent::ThinkingDelta(_) if self.in_model_call() => {
                self.doing = Doing::Thinking;
            }
            AgentLoopEvent::TextDelta(_) | AgentLoopEvent::ToolInputSnapshot { .. }
                if self.in_model_call() =>
            {
                self.doing = Doing::Writing;
            }
            AgentLoopEvent::ToolStart { id, name } => {
                if !self.open_tools.iter().any(|(open, _)| open == id) {
                    self.open_tools.push((id.clone(), name.clone()));
                }
                if self.in_model_call() {
                    self.doing = Doing::Writing;
                }
            }
            AgentLoopEvent::StreamReset { reason, .. } => {
                self.restarts += 1;
                self.restart_reason = Some(reason.clone());
                self.doing = Doing::Waiting;
                self.open_tools.clear();
            }
            AgentLoopEvent::Debug(DebugEvent::ModelExchange { .. }) => {
                self.calls += 1;
                self.restarts = 0;
                self.restart_reason = None;
                self.since_ms = at_ms;
                self.doing = self.tools_running();
                if self.doing == Doing::Waiting {
                    self.next_call(at_ms);
                }
            }
            AgentLoopEvent::ToolResult { tool_use_id, .. } => {
                self.open_tools.retain(|(id, _)| id != tool_use_id);
                if self.open_tools.is_empty() && matches!(self.doing, Doing::Tools(_)) {
                    self.next_call(at_ms);
                } else if let Doing::Tools(_) = self.doing {
                    self.doing = self.tools_running();
                }
            }
            _ => {}
        }
    }

    fn in_model_call(&self) -> bool {
        !matches!(self.doing, Doing::Tools(_))
    }

    fn tools_running(&self) -> Doing {
        if self.open_tools.is_empty() {
            Doing::Waiting
        } else {
            Doing::Tools(self.open_tools.iter().map(|(_, n)| n.clone()).collect())
        }
    }

    fn next_call(&mut self, at_ms: u64) {
        self.doing = Doing::Waiting;
        self.since_ms = at_ms;
        self.restarts = 0;
        self.restart_reason = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::trace::{ModelAction, ModelExchange, TokenUsage};

    fn exchange() -> AgentLoopEvent {
        AgentLoopEvent::Debug(DebugEvent::ModelExchange {
            timestamp: chrono::Utc::now(),
            exchange: Box::new(ModelExchange {
                iteration: 0,
                model: "m".into(),
                observation: None,
                action: ModelAction::default(),
                usage: TokenUsage::default(),
                latency_ms: 0,
                provider_request_id: None,
            }),
            task_id: None,
        })
    }

    fn tool_start(id: &str, name: &str) -> AgentLoopEvent {
        AgentLoopEvent::ToolStart {
            id: id.into(),
            name: name.into(),
        }
    }

    fn tool_result(id: &str) -> AgentLoopEvent {
        AgentLoopEvent::ToolResult {
            tool_use_id: id.into(),
            tool_name: "t".into(),
            content: String::new(),
            is_error: false,
            image: None,
        }
    }

    #[test]
    fn a_call_reasons_writes_then_its_tools_run_and_the_next_call_starts() {
        let mut a = Activity::default();
        a.turn_started(1_000);
        assert_eq!((a.doing.clone(), a.since_ms), (Doing::Waiting, 1_000));
        a.on_event(&AgentLoopEvent::ThinkingDelta("hm".into()), 2_000);
        assert_eq!(a.doing, Doing::Thinking);
        a.on_event(&tool_start("t1", "run_command"), 3_000);
        a.on_event(&tool_start("t2", "task"), 3_100);
        assert_eq!(a.doing, Doing::Writing);
        assert_eq!(a.since_ms, 1_000, "the call's clock runs from its start");
        a.on_event(&exchange(), 4_000);
        assert_eq!(
            (a.doing.clone(), a.since_ms, a.calls),
            (
                Doing::Tools(vec!["run_command".into(), "task".into()]),
                4_000,
                1
            )
        );
        a.on_event(&tool_result("t1"), 5_000);
        assert_eq!(a.doing, Doing::Tools(vec!["task".into()]));
        a.on_event(&tool_result("t2"), 9_000);
        assert_eq!((a.doing.clone(), a.since_ms), (Doing::Waiting, 9_000));
    }

    #[test]
    fn restarts_count_until_the_call_finishes() {
        let mut a = Activity::default();
        a.turn_started(0);
        let reset = AgentLoopEvent::StreamReset {
            reason: "cut off".into(),
            text_bytes: 0,
            thinking_bytes: 10,
        };
        a.on_event(&AgentLoopEvent::ThinkingDelta("x".into()), 1);
        a.on_event(&reset, 120_000);
        a.on_event(&reset, 240_000);
        assert_eq!((a.restarts, a.since_ms), (2, 0));
        assert_eq!(a.restart_reason.as_deref(), Some("cut off"));
        a.on_event(&exchange(), 300_000);
        assert_eq!((a.restarts, a.restart_reason.clone()), (0, None));
        assert_eq!((a.doing.clone(), a.since_ms), (Doing::Waiting, 300_000));
    }
}
