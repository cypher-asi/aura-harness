use super::*;

fn base_request(mode: &str) -> GenerationRequest {
    GenerationRequest {
        mode: mode.to_string(),
        prompt: None,
        model: None,
        size: None,
        quality: None,
        image_url: None,
        images: None,
        project_id: None,
        parent_id: None,
        is_iteration: None,
        aspect_ratio: None,
        duration_seconds: None,
        resolution: None,
        generate_audio: None,
    }
}

#[test]
fn image_request_forwards_quality() {
    let mut req = base_request("image");
    req.prompt = Some("a cat".into());
    req.model = Some("gpt-image-2".into());
    req.quality = Some("high".into());

    let (url, body) = build_router_request("http://router", &req).expect("image request");

    assert_eq!(url, "http://router/v1/generate-image/stream");
    assert_eq!(body["quality"], "high");
    assert_eq!(body["model"], "gpt-image-2");
}

#[test]
fn video_request_forwards_source_images() {
    let mut req = base_request("video");
    req.prompt = Some("a cat surfing".into());
    req.images = Some(vec!["data:image/png;base64,abc".into()]);

    let (url, body) = build_router_request("http://router", &req).expect("video request");

    assert_eq!(url, "http://router/v1/generate-video/stream");
    assert_eq!(body["images"][0], "data:image/png;base64,abc");
}

#[test]
fn three_d_request_forwards_model_and_parent_id() {
    let mut req = base_request("3d");
    req.image_url = Some("https://cdn.example.test/cat.png".into());
    req.model = Some("tripo-v2".into());
    req.parent_id = Some("art-123".into());

    let (url, body) = build_router_request("http://router", &req).expect("3d request");

    assert_eq!(url, "http://router/v1/generate-3d/stream");
    assert_eq!(body["model"], "tripo-v2");
    assert_eq!(body["parentId"], "art-123");
}

#[test]
fn pop_sse_frame_handles_lf_and_crlf_boundaries() {
    let mut buffer = "event: start\ndata: {}\n\nevent: done\r\ndata: {}\r\n\r\ntail".to_string();

    assert_eq!(
        pop_sse_frame(&mut buffer).as_deref(),
        Some("event: start\ndata: {}")
    );
    assert_eq!(
        pop_sse_frame(&mut buffer).as_deref(),
        Some("event: done\r\ndata: {}")
    );
    assert_eq!(buffer, "tail");
    assert!(pop_sse_frame(&mut buffer).is_none());
}

#[test]
fn parse_sse_frame_accepts_fields_without_space_after_colon() {
    let frame = r#"event:progress
data:{"percent":42,"message":"working"}"#;

    let (event_type, msg) = parse_sse_frame(frame, "image").expect("progress frame");

    assert_eq!(event_type, "progress");
    match msg {
        OutboundMessage::GenerationProgress(progress) => {
            assert_eq!(progress.percent, 42.0);
            assert_eq!(progress.message, "working");
        }
        other => panic!("expected progress message, got {other:?}"),
    }
}

#[test]
fn parse_sse_frame_uses_json_type_when_event_line_is_missing() {
    let frame = r#"data: {"type":"completed","imageUrl":"https://example.test/image.png"}"#;

    let (event_type, msg) = parse_sse_frame(frame, "image").expect("completed frame");

    assert_eq!(event_type, "completed");
    match msg {
        OutboundMessage::GenerationCompleted(completed) => {
            assert_eq!(completed.mode, "image");
            assert_eq!(
                completed
                    .payload
                    .get("imageUrl")
                    .and_then(|value| value.as_str()),
                Some("https://example.test/image.png")
            );
        }
        other => panic!("expected completed message, got {other:?}"),
    }
}

#[test]
fn parse_positive_secs_accepts_trimmed_positive_values() {
    assert_eq!(parse_positive_secs(Some("300")), Some(300));
    assert_eq!(parse_positive_secs(Some("  450  ")), Some(450));
}

#[test]
fn parse_positive_secs_rejects_empty_zero_and_invalid_values() {
    assert_eq!(parse_positive_secs(None), None);
    assert_eq!(parse_positive_secs(Some("")), None);
    assert_eq!(parse_positive_secs(Some("0")), None);
    assert_eq!(parse_positive_secs(Some("not-a-number")), None);
}
