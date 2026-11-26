use super::process_support::*;
use super::support::*;
use crate::gateway::*;
use axum::body::Body;
use axum::http::StatusCode;
use base64::Engine as _;
use tower::util::ServiceExt;

// ============================================================================
// File handler security tests (security audit — phase 3)
// ============================================================================
//
// These tests exercise `/api/read-file` and `/api/files` against a real
// on-disk workspace under `TempDir`, asserting that:
//   * canonicalized path traversal is rejected (403 Forbidden),
//   * legitimate in-workspace reads succeed with the file contents,
//   * oversize files trip the 5 MiB cap with 413 Payload Too Large.
//
// The previous `Path::starts_with` check on the unresolved input was
// bypassable with `../` sequences — these tests pin the behaviour of
// the canonicalizing `NodeConfig::resolve_allowed_path` replacement.

#[tokio::test]
async fn test_read_file_rejects_path_traversal() {
    let (state, tmp) = test_router_state_with_workspace();
    let workspaces = tmp.path().join("workspaces");
    // Drop a secret file *outside* the workspace root to serve as the
    // target of the traversal attempt. The canonical resolver should
    // refuse to expose it even though it's a real, readable file on
    // disk under `data_dir/`.
    let secret = tmp.path().join("secret.txt");
    std::fs::write(&secret, "top-secret").unwrap();
    // Also drop a decoy file inside `workspaces` so the traversal has
    // a valid starting segment to anchor against.
    std::fs::write(workspaces.join("decoy.txt"), "ok").unwrap();

    let app = create_router(state);

    // `workspaces/../secret.txt` canonicalises to `<tmp>/secret.txt`
    // which is one level above the root — must be rejected.
    let traversal = format!(
        "{}/../secret.txt",
        workspaces.to_string_lossy().replace('\\', "/")
    );
    let uri = format!("/api/read-file?path={}", urlencode(&traversal));
    let req = authed_request().uri(uri).body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "traversal must be refused, not return the secret"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains("top-secret"),
        "response must not leak file contents; got: {text}"
    );
}

#[tokio::test]
async fn test_read_file_returns_workspace_file() {
    let (state, tmp) = test_router_state_with_workspace();
    let workspaces = tmp.path().join("workspaces");
    let file_path = workspaces.join("hello.txt");
    std::fs::write(&file_path, "hello, world").unwrap();

    let app = create_router(state);

    let uri = format!(
        "/api/read-file?path={}",
        urlencode(&file_path.to_string_lossy())
    );
    let req = authed_request().uri(uri).body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["content"], "hello, world");
    assert_eq!(
        json["revision"],
        blake3::hash(b"hello, world").to_hex().to_string()
    );
}

#[tokio::test]
async fn test_write_file_replaces_matching_workspace_file() {
    let (state, tmp) = test_router_state_with_workspace();
    let file_path = tmp.path().join("workspaces").join("hello.txt");
    std::fs::write(&file_path, "before").unwrap();
    let app = create_router(state);
    let body = serde_json::json!({
        "path": file_path,
        "content_base64": base64::engine::general_purpose::STANDARD.encode("after"),
        "expected_revision": blake3::hash(b"before").to_hex().to_string(),
    });

    let request = authed_request()
        .method("PUT")
        .uri("/api/write-file")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(std::fs::read_to_string(file_path).unwrap(), "after");
}

#[tokio::test]
async fn test_write_file_rejects_stale_content_without_overwriting() {
    let (state, tmp) = test_router_state_with_workspace();
    let file_path = tmp.path().join("workspaces").join("hello.txt");
    std::fs::write(&file_path, "agent edit").unwrap();
    let app = create_router(state);
    let body = serde_json::json!({
        "path": file_path,
        "content_base64": base64::engine::general_purpose::STANDARD.encode("browser edit"),
        "expected_revision": blake3::hash(b"old content").to_hex().to_string(),
    });

    let request = authed_request()
        .method("PUT")
        .uri("/api/write-file")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(std::fs::read_to_string(file_path).unwrap(), "agent edit");
}

#[tokio::test]
async fn test_write_file_rejects_path_traversal() {
    let (state, tmp) = test_router_state_with_workspace();
    let secret = tmp.path().join("secret.txt");
    std::fs::write(&secret, "secret").unwrap();
    let app = create_router(state);
    let body = serde_json::json!({
        "path": secret,
        "content_base64": base64::engine::general_purpose::STANDARD.encode("overwritten"),
        "expected_revision": blake3::hash(b"secret").to_hex().to_string(),
    });

    let request = authed_request()
        .method("PUT")
        .uri("/api/write-file")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(std::fs::read_to_string(secret).unwrap(), "secret");
}

#[tokio::test]
async fn test_read_file_caps_at_5mib() {
    let (state, tmp) = test_router_state_with_workspace();
    let workspaces = tmp.path().join("workspaces");
    // 5 MiB + 1 byte — one byte over the cap is the minimum signal
    // that the cap is enforced and not off-by-one in our favour.
    let oversize = workspaces.join("big.bin");
    let payload = vec![b'A'; 5 * 1024 * 1024 + 1];
    std::fs::write(&oversize, &payload).unwrap();

    let app = create_router(state);

    let uri = format!(
        "/api/read-file?path={}",
        urlencode(&oversize.to_string_lossy())
    );
    let req = authed_request().uri(uri).body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "oversize reads must trip the 5 MiB cap, not OOM the process"
    );
}

#[tokio::test]
async fn test_list_files_rejects_path_traversal() {
    let (state, tmp) = test_router_state_with_workspace();
    let workspaces = tmp.path().join("workspaces");
    std::fs::write(workspaces.join("inside.txt"), "ok").unwrap();

    let app = create_router(state);

    let traversal = format!("{}/../..", workspaces.to_string_lossy().replace('\\', "/"));
    let uri = format!("/api/files?path={}", urlencode(&traversal));
    let req = authed_request().uri(uri).body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "listing above the workspace root must be refused"
    );
}

#[tokio::test]
async fn test_workspace_import_and_delete_use_stable_workspace_key() {
    let (state, tmp) = test_router_state_with_workspace();
    let app = create_router(state);
    let workspace_key = "36d4494f-75df-4c02-84d5-07aef06d2569";
    let body = serde_json::json!({
        "workspace_key": workspace_key,
        "files": [
            {
                "relative_path": "src/main.ts",
                "contents_base64": base64::engine::general_purpose::STANDARD.encode("hello"),
            },
            {
                "relative_path": "README.md",
                "contents_base64": base64::engine::general_purpose::STANDARD.encode("read me"),
            }
        ]
    });
    let request = authed_request()
        .method("POST")
        .uri("/workspace/import")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let workspace = tmp.path().join("workspaces").join(workspace_key);
    assert_eq!(
        std::fs::read_to_string(workspace.join("src/main.ts")).unwrap(),
        "hello"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("README.md")).unwrap(),
        "read me"
    );

    let request = authed_request()
        .method("DELETE")
        .uri(format!("/workspace/{workspace_key}"))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(!workspace.exists());
}

/// Prepare a safe workspace for `session_id` and return its isolated path.
async fn prepare_safe_workspace(
    app: &axum::Router,
    workspace_key: &str,
    session_id: &str,
) -> std::path::PathBuf {
    let uri = format!("/workspace/{workspace_key}/safe/{session_id}");
    let (status, prepared) = send_json(app, "POST", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    std::path::PathBuf::from(prepared["workspacePath"].as_str().unwrap())
}

/// Fetch the safe-workspace status and return its first checkpoint id.
async fn first_safe_checkpoint(
    app: &axum::Router,
    workspace_key: &str,
    session_id: &str,
) -> String {
    let uri = format!("/workspace/{workspace_key}/safe/{session_id}");
    let (response_status, bytes) = send(app, "GET", &uri, None).await;
    assert_eq!(
        response_status,
        StatusCode::OK,
        "status response: {}",
        String::from_utf8_lossy(&bytes)
    );
    let status: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status["enabled"], true);
    status["checkpoints"][0]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_hosted_safe_workspace_prepare_diff_and_apply_round_trip() {
    let (state, tmp) = test_router_state_with_workspace();
    let app = create_router(state);
    let workspace_key = "36d4494f-75df-4c02-84d5-07aef06d2569";
    let session_id = "c4e91e27-cde4-4d66-a077-51d88c987c43";
    let source = tmp.path().join("workspaces").join(workspace_key);
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("app.txt"), "source\n").unwrap();

    let isolated = prepare_safe_workspace(&app, workspace_key, session_id).await;
    assert_eq!(
        std::fs::read_to_string(isolated.join("app.txt")).unwrap(),
        "source\n"
    );
    std::fs::write(isolated.join("app.txt"), "isolated\n").unwrap();
    assert_eq!(
        std::fs::read_to_string(source.join("app.txt")).unwrap(),
        "source\n"
    );

    let checkpoint = first_safe_checkpoint(&app, workspace_key, session_id).await;

    let uri = format!("/workspace/{workspace_key}/safe/{session_id}/checkpoints/{checkpoint}/diff");
    let (status, diff) = send_json(&app, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(diff["diff"].as_str().unwrap().contains("isolated"));

    let uri = format!("/workspace/{workspace_key}/safe/{session_id}/apply");
    assert_eq!(send_status(&app, "POST", &uri, None).await, StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(source.join("app.txt")).unwrap(),
        "isolated\n"
    );

    let uri = format!("/workspace/{workspace_key}");
    let status = send_status(&app, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!tmp
        .path()
        .join("safe-workspaces")
        .join(workspace_key)
        .exists());
}

#[tokio::test]
async fn test_workspace_import_rejects_path_traversal() {
    let (state, tmp) = test_router_state_with_workspace();
    let app = create_router(state);
    let body = serde_json::json!({
        "workspace_key": "safe-project-id",
        "files": [{
            "relative_path": "../outside.txt",
            "contents_base64": base64::engine::general_purpose::STANDARD.encode("secret"),
        }]
    });
    let request = authed_request()
        .method("POST")
        .uri("/workspace/import")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!tmp.path().join("outside.txt").exists());
}

#[tokio::test]
async fn test_workspace_lifecycle_rejects_key_that_slugifies_to_root() {
    let (state, tmp) = test_router_state_with_workspace();
    let app = create_router(state);
    let body = serde_json::json!({
        "workspace_key": "...",
        "files": [{
            "relative_path": "outside.txt",
            "contents_base64": base64::engine::general_purpose::STANDARD.encode("secret"),
        }]
    });
    let request = authed_request()
        .method("POST")
        .uri("/workspace/import")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!tmp.path().join("workspaces/outside.txt").exists());

    let request = authed_request()
        .method("DELETE")
        .uri("/workspace/...")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(tmp.path().join("workspaces").is_dir());
}
