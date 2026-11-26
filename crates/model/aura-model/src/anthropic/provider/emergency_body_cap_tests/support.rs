use serde_json::json;

pub(super) fn body_with_user_text(text: &str) -> Vec<u8> {
    let body = json!({
        "model": "aura-claude-opus-4-7",
        "system": [{"type": "text", "text": "system prompt"}],
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "earlier turn"}
                ]
            },
            {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "ok"}
                ]
            },
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": text}
                ]
            }
        ],
        "max_tokens": 1024,
        "stream": true,
    });
    serde_json::to_vec(&body).expect("serialize")
}

/// Two user turns with images: a large older one and a small current one.
pub(super) fn body_with_older_and_current_image() -> Vec<u8> {
    serde_json::to_vec(&json!({
            "model": "aura-claude-opus-4-7",
            "system": [{"type": "text", "text": "system prompt"}],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "make the homepage look like this"},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "A".repeat(200_000)}}
                    ]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "got it, here is my plan"}]
                },
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "also use this reference"},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "B".repeat(10_000)}}
                    ]
                }
            ],
            "max_tokens": 1024,
            "stream": true,
        }))
        .unwrap()
}

/// Image-heavy first user turn (mirrors the real incident where two
/// attached images pushed the body over the cap), then several tool-call
/// rounds.
pub(super) fn body_with_image_then_tool_rounds() -> Vec<u8> {
    let mut messages = vec![json!({
        "role": "user",
        "content": [
            {"type": "text", "text": "make the homepage look like this"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "A".repeat(120_000)}}
        ]
    })];
    for i in 0..6 {
        let id = format!("toolu_{i}");
        messages.push(json!({
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": id, "name": "get_project", "input": {}}
            ]
        }));
        messages.push(json!({
            "role": "user",
            "content": [
                {"type": "tool_result", "tool_use_id": id, "content": "Z".repeat(30_000)}
            ]
        }));
    }

    serde_json::to_vec(&json!({
        "model": "aura-claude-opus-4-7",
        "system": [{"type": "text", "text": "system prompt"}],
        "messages": messages,
        "max_tokens": 1024,
        "stream": true,
    }))
    .unwrap()
}

/// Anthropic's positional rule: every `tool_result` in message i must
/// reference a `tool_use` in message i-1.
pub(super) fn assert_every_tool_result_follows_its_tool_use(msgs: &[serde_json::Value]) {
    for (i, msg) in msgs.iter().enumerate() {
        let Some(blocks) = msg["content"].as_array() else {
            continue;
        };
        for block in blocks {
            if block["type"].as_str() != Some("tool_result") {
                continue;
            }
            let id = block["tool_use_id"].as_str().unwrap();
            let paired = i > 0
                && msgs[i - 1]["content"].as_array().is_some_and(|prev| {
                    prev.iter().any(|b| {
                        b["type"].as_str() == Some("tool_use") && b["id"].as_str() == Some(id)
                    })
                });
            assert!(
                paired,
                "tool_result {id} at message {i} has no tool_use in the previous message"
            );
        }
    }
}

/// A user instruction, one `read_file` tool round, and a tool result
/// followed by an image.
pub(super) fn body_with_tool_loop_and_image() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "model": "aura-claude-fable-5-1",
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Update the dashboard totals"}
                ]
            },
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "read_file",
                        "input": {"path": "dashboard.tsx"}
                    }
                ]
            },
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_1",
                        "content": "The current total is computed on line 42"
                    },
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "A".repeat(20_000)
                        }
                    }
                ]
            }
        ],
        "max_tokens": 1024,
        "stream": true,
    }))
    .unwrap()
}
