//! Responses API stream events → [`StreamEvent`].
//!
//! Reasoning items are opaque and must be replayed whole on the next
//! turn (`store: false`), so they are collected and emitted as a single
//! thinking block at the end whose signature carries the items.

use std::collections::HashSet;

use serde_json::Value;

use super::encode_reasoning;
use crate::{StopReason, StreamContentType, StreamEvent};

/// Index used for the synthetic thinking block; real blocks use the
/// provider's `output_index`.
const REASONING_INDEX: u32 = u32::MAX;

#[derive(Debug, Default)]
pub(super) struct Decoder {
    model: String,
    started: bool,
    open: HashSet<u32>,
    streamed_args: HashSet<u32>,
    reasoning: Vec<Value>,
    summary: Vec<String>,
    saw_tool_call: bool,
    finished: bool,
}

impl Decoder {
    pub(super) fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            ..Self::default()
        }
    }

    /// Whether a terminal event has been seen.
    pub(super) const fn finished(&self) -> bool {
        self.finished
    }

    /// Events for a non-streamed `response` object (some deployments
    /// answer with JSON even when asked to stream).
    pub(super) fn response_object(&mut self, response: &Value) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        self.start(response, &mut out);
        if let Some(items) = response.get("output").and_then(Value::as_array) {
            for (index, item) in items.iter().enumerate() {
                let index = u32::try_from(index).unwrap_or(u32::MAX - 1);
                self.item_done(index, item, &mut out);
            }
        }
        self.finish(response, &mut out);
        out
    }

    /// Events for one decoded SSE `data:` payload.
    pub(super) fn event(&mut self, event: &Value) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let index = event
            .get("output_index")
            .and_then(Value::as_u64)
            .and_then(|i| u32::try_from(i).ok())
            .unwrap_or(0);
        match kind {
            "response.created" | "response.in_progress" => {
                if let Some(response) = event.get("response") {
                    self.start(response, &mut out);
                }
            }
            "response.output_item.added" => {
                self.start(&Value::Null, &mut out);
                if let Some(item) = event.get("item") {
                    self.open_item(index, item, &mut out);
                }
            }
            "response.output_text.delta" => {
                let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
                if !self.open.contains(&index) {
                    self.open_block(index, StreamContentType::Text, &mut out);
                }
                out.push(StreamEvent::TextDelta {
                    text: delta.to_string(),
                });
            }
            "response.function_call_arguments.delta" => {
                if self.open.contains(&index) {
                    let delta = event.get("delta").and_then(Value::as_str).unwrap_or("");
                    self.streamed_args.insert(index);
                    out.push(StreamEvent::InputJsonDelta {
                        partial_json: delta.to_string(),
                    });
                }
            }
            "response.output_item.done" => {
                if let Some(item) = event.get("item") {
                    self.item_done(index, item, &mut out);
                }
            }
            "response.completed" | "response.incomplete" => {
                let response = event.get("response").unwrap_or(&Value::Null);
                self.finish(response, &mut out);
            }
            "response.failed" => {
                self.finished = true;
                let message = event
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("response failed");
                out.push(error(message));
            }
            "error" => {
                self.finished = true;
                let message = event
                    .get("message")
                    .or_else(|| event.pointer("/error/message"))
                    .and_then(Value::as_str)
                    .unwrap_or("stream error");
                out.push(error(message));
            }
            // Reasoning summaries, content-part bookkeeping and other
            // progress frames still prove the stream is alive.
            _ => out.push(StreamEvent::Ping),
        }
        out
    }

    fn start(&mut self, response: &Value, out: &mut Vec<StreamEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        out.push(StreamEvent::MessageStart {
            message_id: str_at(response, "id").to_string(),
            model: non_empty(str_at(response, "model"))
                .unwrap_or(&self.model)
                .to_string(),
            input_tokens: None,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });
    }

    fn open_block(
        &mut self,
        index: u32,
        content_type: StreamContentType,
        out: &mut Vec<StreamEvent>,
    ) {
        self.open.insert(index);
        out.push(StreamEvent::ContentBlockStart {
            index,
            content_type,
        });
    }

    fn open_item(&mut self, index: u32, item: &Value, out: &mut Vec<StreamEvent>) {
        match str_at(item, "type") {
            "message" => self.open_block(index, StreamContentType::Text, out),
            "function_call" => self.open_block(
                index,
                StreamContentType::ToolUse {
                    id: str_at(item, "call_id").to_string(),
                    name: str_at(item, "name").to_string(),
                },
                out,
            ),
            _ => {}
        }
    }

    fn item_done(&mut self, index: u32, item: &Value, out: &mut Vec<StreamEvent>) {
        match str_at(item, "type") {
            "reasoning" => {
                if let Some(parts) = item.get("summary").and_then(Value::as_array) {
                    self.summary.extend(
                        parts
                            .iter()
                            .filter_map(|p| p.get("text").and_then(Value::as_str))
                            .filter(|t| !t.is_empty())
                            .map(str::to_string),
                    );
                }
                self.reasoning.push(item.clone());
            }
            "function_call" => {
                self.saw_tool_call = true;
                if !self.open.contains(&index) {
                    self.open_item(index, item, out);
                }
                if !self.streamed_args.contains(&index) {
                    out.push(StreamEvent::InputJsonDelta {
                        partial_json: str_at(item, "arguments").to_string(),
                    });
                }
                self.close(index, out);
            }
            "message" => {
                if !self.open.contains(&index) {
                    self.open_block(index, StreamContentType::Text, out);
                    let text: String = item
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter(|part| str_at(part, "type") == "output_text")
                        .map(|part| str_at(part, "text"))
                        .collect();
                    if !text.is_empty() {
                        out.push(StreamEvent::TextDelta { text });
                    }
                }
                self.close(index, out);
            }
            _ => {}
        }
    }

    fn close(&mut self, index: u32, out: &mut Vec<StreamEvent>) {
        if self.open.remove(&index) {
            out.push(StreamEvent::ContentBlockStop { index });
        }
    }

    fn finish(&mut self, response: &Value, out: &mut Vec<StreamEvent>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let mut open: Vec<u32> = self.open.iter().copied().collect();
        open.sort_unstable();
        for index in open {
            self.close(index, out);
        }

        if let Some(signature) = encode_reasoning(&self.reasoning) {
            out.push(StreamEvent::ContentBlockStart {
                index: REASONING_INDEX,
                content_type: StreamContentType::Thinking,
            });
            let summary = self.summary.join("\n\n");
            out.push(StreamEvent::ThinkingDelta {
                thinking: if summary.is_empty() {
                    "(reasoning)".to_string()
                } else {
                    summary
                },
            });
            out.push(StreamEvent::SignatureDelta { signature });
            out.push(StreamEvent::ContentBlockStop {
                index: REASONING_INDEX,
            });
        }

        let usage = response.get("usage");
        let tokens = |pointer: &str| {
            usage
                .and_then(|u| u.pointer(pointer))
                .and_then(Value::as_u64)
        };
        // Input usage is only known at the end; a second start frame
        // carries it to the accumulator.
        out.push(StreamEvent::MessageStart {
            message_id: str_at(response, "id").to_string(),
            model: non_empty(str_at(response, "model"))
                .unwrap_or(&self.model)
                .to_string(),
            input_tokens: tokens("/input_tokens"),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: tokens("/input_tokens_details/cached_tokens"),
        });

        let incomplete_reason = response
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str);
        let stop_reason = if self.saw_tool_call {
            StopReason::ToolUse
        } else if incomplete_reason == Some("max_output_tokens") {
            StopReason::MaxTokens
        } else {
            StopReason::EndTurn
        };
        out.push(StreamEvent::MessageDelta {
            stop_reason: Some(stop_reason),
            output_tokens: tokens("/output_tokens").unwrap_or(0),
        });
        out.push(StreamEvent::MessageStop);
    }
}

fn error(message: &str) -> StreamEvent {
    StreamEvent::Error {
        message: message.to_string(),
        request_id: None,
    }
}

fn str_at<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn non_empty(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}
