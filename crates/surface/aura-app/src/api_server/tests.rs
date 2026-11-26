use super::*;
use axum::body::Body;
use axum::http::Request;
use tower::util::ServiceExt;

/// Build a router identical to the one [`start_api_server`] mounts,
/// but without the TCP bind / spawn — so tests can drive it with
/// `oneshot`. Returns `(router, token, tempdir)` where the
/// `tempdir` is the sandbox root and must be kept alive for the
/// duration of the test.
fn test_app() -> (axum::Router, String, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = Sandbox::new(tmp.path()).unwrap();
    let token = generate_token();
    let state = ApiState {
        expected_token: Arc::new(token.clone()),
        sandbox: Arc::new(sandbox),
    };
    let public = Router::new().route("/health", get(api_health_handler));
    let protected = Router::new()
        .route("/api/files", get(api_list_files_handler))
        .route("/api/read-file", get(api_read_file_handler))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_mw,
        ));
    let app = Router::new()
        .merge(public)
        .merge(protected)
        .with_state(state);
    (app, token, tmp)
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let safe = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/' | b':' | b'\\');
        if safe {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

#[tokio::test]
async fn health_is_anonymous() {
    let (app, _token, _tmp) = test_app();
    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn read_file_without_token_is_unauthorized() {
    let (app, _token, tmp) = test_app();
    std::fs::write(tmp.path().join("ok.txt"), "hi").unwrap();
    let uri = format!(
        "/api/read-file?path={}",
        urlencode(&tmp.path().join("ok.txt").to_string_lossy())
    );
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn read_file_rejects_path_traversal() {
    let (app, token, tmp) = test_app();
    // Secret lives one level *above* the sandbox root so any
    // traversal attempt that resolves to it must fail.
    let parent = tmp.path().parent().unwrap();
    let secret = parent.join(format!(
        "aura-api-server-secret-{}.txt",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&secret, "top-secret").unwrap();

    let uri = format!(
        "/api/read-file?path={}",
        urlencode(&secret.to_string_lossy())
    );
    let req = Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // Cleanup before asserting so the file doesn't leak when the
    // test is re-run.
    let _ = std::fs::remove_file(&secret);
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn read_file_returns_sandboxed_file() {
    let (app, token, tmp) = test_app();
    let path = tmp.path().join("hello.txt");
    std::fs::write(&path, "hello, world").unwrap();

    let uri = format!("/api/read-file?path={}", urlencode(&path.to_string_lossy()));
    let req = Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["content"], "hello, world");
}

#[tokio::test]
async fn read_file_caps_oversize() {
    let (app, token, tmp) = test_app();
    let path = tmp.path().join("big.bin");
    let payload = vec![b'A'; 5 * 1024 * 1024 + 1];
    std::fs::write(&path, &payload).unwrap();

    let uri = format!("/api/read-file?path={}", urlencode(&path.to_string_lossy()));
    let req = Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

// The constant-time bearer compare now lives in
// `aura_auth_kit::bearer::check_bearer` and is exercised by that crate's
// test suite. No duplicated test kept here — see
// `crates/auth/aura-auth-kit/src/bearer/tests.rs` for coverage.
