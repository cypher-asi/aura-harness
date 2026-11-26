use super::*;
use crate::web::test_server::{Reply, TestServer};
use crate::{Sandbox, ToolConfig};

fn tool() -> WebFetchTool {
    WebFetchTool::new(WebFetchSettings {
        allow_private: true,
    })
    .unwrap()
}

async fn run(tool: &WebFetchTool, args: serde_json::Value) -> ToolResult {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(Sandbox::new(dir.path()).unwrap(), ToolConfig::default());
    tool.execute(&ctx, args).await.unwrap()
}

fn text(result: &ToolResult) -> String {
    let bytes = if result.ok {
        &result.stdout
    } else {
        &result.stderr
    };
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test]
async fn converts_html_and_follows_redirects() {
    let server = TestServer::start(vec![
            ("/old", Reply::redirect("/page")),
            (
                "/page",
                Reply::ok(
                    "text/html; charset=utf-8",
                    "<html><head><title>Docs</title><script>x()</script></head><body>\
                     <nav>menu</nav><h1>Hello</h1><p>See <a href=\"/next\">next</a>.</p></body></html>",
                ),
            ),
        ])
        .await;
    let result = run(&tool(), serde_json::json!({"url": server.url("/old")})).await;
    let out = text(&result);
    assert!(result.ok, "{out}");
    assert!(
        out.starts_with(&format!("URL: {}\nTitle: Docs\n", server.url("/page"))),
        "{out}"
    );
    assert!(
        out.contains(&format!("# Hello\n\nSee [next]({}).", server.url("/next"))),
        "{out}"
    );
    assert!(!out.contains("menu") && !out.contains("x()"), "{out}");
    let ua = server.requests()[0]
        .header("user-agent")
        .unwrap_or_default()
        .to_string();
    assert!(ua.starts_with("aura-harness/"), "{ua}");
}

#[tokio::test]
async fn plain_text_and_json_pass_through() {
    let server = TestServer::start(vec![(
        "/data.json",
        Reply::ok("application/json", r#"{"a": "<b>"}"#),
    )])
    .await;
    let out = text(
        &run(
            &tool(),
            serde_json::json!({"url": server.url("/data.json")}),
        )
        .await,
    );
    assert!(out.ends_with("\n\n{\"a\": \"<b>\"}"), "{out}");
}

#[tokio::test]
async fn refuses_binary_content() {
    let server = TestServer::start(vec![
        (
            "/logo.png",
            Reply::ok("image/png", vec![0x89, b'P', b'N', b'G', 0]),
        ),
        ("/blob", {
            let mut r = Reply::ok("", vec![0u8, 1, 2, 3]);
            r.headers.clear();
            r
        }),
    ])
    .await;
    let result = run(&tool(), serde_json::json!({"url": server.url("/logo.png")})).await;
    assert!(!result.ok);
    assert!(text(&result).contains("returned `image/png`; web_fetch only reads text"));
    let result = run(&tool(), serde_json::json!({"url": server.url("/blob")})).await;
    assert!(!result.ok);
    assert!(text(&result).contains("binary data"), "{}", text(&result));
}

#[tokio::test]
async fn reports_http_errors_with_an_excerpt() {
    let server = TestServer::start(vec![]).await;
    let result = run(&tool(), serde_json::json!({"url": server.url("/missing")})).await;
    assert!(!result.ok);
    let out = text(&result);
    assert!(
        out.contains("returned HTTP 404") && out.contains("not found"),
        "{out}"
    );
}

#[tokio::test]
async fn pages_through_long_content() {
    let body = "a".repeat(2_500);
    let server = TestServer::start(vec![("/long", Reply::ok("text/plain", body))]).await;
    let url = server.url("/long");
    let out = text(&run(&tool(), serde_json::json!({"url": url, "max_chars": 1000})).await);
    assert!(
        out.contains(
            "[showing characters 0..1000 of 2500; call web_fetch again with start_index=1000"
        ),
        "{out}"
    );
    let out = text(
        &run(
            &tool(),
            serde_json::json!({"url": url, "max_chars": 1000, "start_index": 2000}),
        )
        .await,
    );
    assert!(out.ends_with(&"a".repeat(500)), "{out}");
}

#[tokio::test]
async fn redirect_to_a_private_address_is_refused() {
    let server = TestServer::start(vec![(
        "/hop",
        Reply::redirect("http://169.254.169.254/latest/meta-data"),
    )])
    .await;
    // The test server is itself on loopback, so only the redirect hop
    // is checked with private addresses blocked.
    let strict = WebFetchTool {
        client: fetch_client(false).unwrap(),
        allow_private: true,
    };
    let result = run(&strict, serde_json::json!({"url": server.url("/hop")})).await;
    assert!(!result.ok);
    let out = text(&result);
    assert!(
        out.contains("redirect refused") && out.contains("169.254.169.254"),
        "{out}"
    );
}

#[tokio::test]
async fn invalid_urls_fail_before_any_request() {
    let strict = WebFetchTool::new(WebFetchSettings::default()).unwrap();
    let out = text(&run(&strict, serde_json::json!({"url": "file:///etc/passwd"})).await);
    assert!(out.contains("only http and https"), "{out}");
    let out = text(
        &run(
            &strict,
            serde_json::json!({"url": "http://localhost:8080/"}),
        )
        .await,
    );
    assert!(out.contains("AURA_WEB_FETCH_ALLOW_PRIVATE"), "{out}");
}

#[test]
fn classifies_content_types() {
    assert_eq!(classify(Some("text/html; charset=UTF-8")), Kind::Html);
    assert_eq!(classify(Some("application/ld+json")), Kind::Text);
    assert_eq!(classify(Some("text/markdown")), Kind::Text);
    assert_eq!(classify(None), Kind::Unknown);
    assert_eq!(
        classify(Some("application/pdf")),
        Kind::Binary("application/pdf".into())
    );
}

#[test]
fn decodes_latin1_when_declared() {
    assert_eq!(
        decode(
            &[0x63, 0x61, 0x66, 0xe9],
            Some("text/plain; charset=ISO-8859-1")
        ),
        "café"
    );
    assert_eq!(decode("café".as_bytes(), Some("text/plain")), "café");
}

#[test]
fn settings_read_the_private_flag() {
    assert!(!WebFetchSettings::from_lookup(|_| None).allow_private);
    assert!(WebFetchSettings::from_lookup(|_| Some("1".into())).allow_private);
    assert!(!WebFetchSettings::from_lookup(|_| Some("no".into())).allow_private);
}
