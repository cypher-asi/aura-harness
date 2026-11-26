use super::*;
use crate::web::test_server::{Reply, TestServer};
use crate::{Sandbox, ToolConfig};
use std::collections::HashMap;

fn settings(vars: &[(&str, &str)]) -> WebSearchSettings {
    let vars: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    WebSearchSettings::from_lookup(|name| vars.get(name).cloned())
}

async fn run(tool: &WebSearchTool, args: serde_json::Value) -> ToolResult {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(Sandbox::new(dir.path()).unwrap(), ToolConfig::default());
    tool.execute(&ctx, args).await.unwrap()
}

fn output(result: &ToolResult) -> String {
    let bytes = if result.ok {
        &result.stdout
    } else {
        &result.stderr
    };
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn selects_the_first_configured_provider() {
    assert_eq!(
        settings(&[("TAVILY_API_KEY", "t"), ("BRAVE_API_KEY", "b")]).provider,
        Ok(ProviderChoice::Brave {
            api_key: "b".into()
        })
    );
    assert_eq!(
        settings(&[("BRAVE_SEARCH_API_KEY", "b2")]).provider,
        Ok(ProviderChoice::Brave {
            api_key: "b2".into()
        })
    );
    assert_eq!(
        settings(&[
            ("AURA_SEARXNG_URL", "http://localhost:8888"),
            ("TAVILY_API_KEY", "t")
        ])
        .provider,
        Ok(ProviderChoice::Tavily {
            api_key: "t".into()
        })
    );
    assert_eq!(
        settings(&[("AURA_SEARXNG_URL", "http://localhost:8888")]).provider_name(),
        Some("SearXNG")
    );
}

#[test]
fn explicit_provider_wins_and_needs_its_key() {
    assert_eq!(
        settings(&[
            ("AURA_WEB_SEARCH_PROVIDER", "Tavily"),
            ("BRAVE_API_KEY", "b"),
            ("TAVILY_API_KEY", "t")
        ])
        .provider,
        Ok(ProviderChoice::Tavily {
            api_key: "t".into()
        })
    );
    let err = settings(&[
        ("AURA_WEB_SEARCH_PROVIDER", "brave"),
        ("TAVILY_API_KEY", "t"),
    ])
    .provider
    .unwrap_err();
    assert!(err.contains("BRAVE_API_KEY is not set"), "{err}");
    let err = settings(&[("AURA_WEB_SEARCH_PROVIDER", "bing")])
        .provider
        .unwrap_err();
    assert!(
        err.contains("unknown AURA_WEB_SEARCH_PROVIDER `bing`"),
        "{err}"
    );
    assert!(
        settings(&[("AURA_WEB_SEARCH_PROVIDER", "off"), ("BRAVE_API_KEY", "b")])
            .provider
            .is_err()
    );
}

#[test]
fn blank_keys_count_as_unset_and_keys_stay_out_of_debug() {
    let s = settings(&[("BRAVE_API_KEY", "  ")]);
    assert_eq!(s.provider, Err(NOT_CONFIGURED.to_string()));
    let s = settings(&[("BRAVE_API_KEY", "secret-key")]);
    assert!(!format!("{s:?}").contains("secret-key"));
}

#[tokio::test]
async fn unconfigured_tool_explains_how_to_configure() {
    let tool = WebSearchTool::new(&settings(&[]));
    let result = run(&tool, serde_json::json!({"query": "rust"})).await;
    assert!(!result.ok);
    let out = output(&result);
    assert!(
        out.contains("not configured") && out.contains("BRAVE_API_KEY"),
        "{out}"
    );
}

#[tokio::test]
async fn brave_sends_the_key_and_cleans_snippets() {
    let server = TestServer::start(vec![(
            "/brave",
            Reply::ok(
                "application/json",
                r#"{"web":{"results":[
                    {"title":"The <strong>Rust</strong> Book","url":"https://doc.rust-lang.org/book/","description":"Learn &amp; <strong>build</strong>"},
                    {"title":"","url":"https://example.com/"}
                ]}}"#,
            ),
        )])
        .await;
    let provider = BraveSearch::new("brave-key")
        .unwrap()
        .with_endpoint(server.url("/brave"));
    let tool = WebSearchTool::with_provider(Arc::new(provider));
    let result = run(
        &tool,
        serde_json::json!({"query": "rust book", "count": 50}),
    )
    .await;
    let out = output(&result);
    assert!(result.ok, "{out}");
    assert_eq!(
        out,
        "Results for \"rust book\" (Brave Search):\n\
             \n1. The Rust Book\n   https://doc.rust-lang.org/book/\n   Learn & build\n\
             \n2. https://example.com/\n   https://example.com/\n\
             \nUse web_fetch on a URL to read the page."
    );
    let seen = &server.requests()[0];
    assert_eq!(seen.header("x-subscription-token"), Some("brave-key"));
    assert!(
        seen.path_and_query.contains("q=rust+book"),
        "{}",
        seen.path_and_query
    );
    assert!(
        seen.path_and_query.contains("count=20"),
        "{}",
        seen.path_and_query
    );
}

#[tokio::test]
async fn tavily_posts_json_with_a_bearer_token() {
    let server = TestServer::start(vec![(
        "/tavily",
        Reply::ok(
            "application/json",
            r#"{"results":[{"title":"Tokio","url":"https://tokio.rs","content":"Async runtime"}]}"#,
        ),
    )])
    .await;
    let provider = TavilySearch::new("tvly-key")
        .unwrap()
        .with_endpoint(server.url("/tavily"));
    let hits = provider.search("tokio", 3).await.unwrap();
    assert_eq!(
        hits,
        vec![SearchHit {
            title: "Tokio".into(),
            url: "https://tokio.rs".into(),
            snippet: "Async runtime".into()
        }]
    );
    let seen = &server.requests()[0];
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.header("authorization"), Some("Bearer tvly-key"));
    let body: serde_json::Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(body["query"], "tokio");
    assert_eq!(body["max_results"], 3);
}

#[tokio::test]
async fn searxng_limits_results_and_explains_a_disabled_json_api() {
    let server = TestServer::start(vec![(
            "/search",
            Reply::ok(
                "application/json",
                r#"{"results":[{"url":"https://a.example","title":"A","content":"a"},{"url":"https://b.example","title":"B","content":"b"}]}"#,
            ),
        )])
        .await;
    let provider = SearxngSearch::new(&server.url("/")).unwrap();
    let hits = provider.search("x", 1).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert!(server.requests()[0].path_and_query.contains("format=json"));

    let forbidden = TestServer::start(vec![("/search", Reply::status(403, "Forbidden"))]).await;
    let provider = SearxngSearch::new(&forbidden.url("")).unwrap();
    let err = provider.search("x", 5).await.unwrap_err().to_string();
    assert!(
        err.contains("HTTP 403") && err.contains("search.formats"),
        "{err}"
    );
}

#[tokio::test]
async fn provider_errors_become_tool_failures() {
    let server = TestServer::start(vec![
        ("/limited", Reply::status(429, "slow down")),
        ("/garbage", Reply::ok("application/json", "not json")),
    ])
    .await;
    let limited = BraveSearch::new("k")
        .unwrap()
        .with_endpoint(server.url("/limited"));
    let result = run(
        &WebSearchTool::with_provider(Arc::new(limited)),
        serde_json::json!({"query": "x"}),
    )
    .await;
    assert!(!result.ok);
    assert!(
        output(&result).contains("HTTP 429 (rate limited"),
        "{}",
        output(&result)
    );

    let garbage = BraveSearch::new("k")
        .unwrap()
        .with_endpoint(server.url("/garbage"));
    let err = garbage.search("x", 1).await.unwrap_err().to_string();
    assert!(
        err.contains("Brave Search returned an unexpected response"),
        "{err}"
    );
}

#[tokio::test]
async fn slow_providers_time_out() {
    let mut slow = Reply::ok("application/json", "{}");
    slow.delay = Some(std::time::Duration::from_secs(5));
    let server = TestServer::start(vec![("/slow", slow)]).await;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(200))
        .build()
        .unwrap();
    let provider = BraveSearch {
        client,
        api_key: "k".into(),
        endpoint: server.url("/slow"),
    };
    let err = provider.search("x", 1).await.unwrap_err();
    assert!(matches!(err, WebError::Timeout { .. }), "{err}");
}

#[tokio::test]
async fn empty_query_is_rejected() {
    let tool = WebSearchTool::new(&settings(&[]));
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(Sandbox::new(dir.path()).unwrap(), ToolConfig::default());
    let err = tool
        .execute(&ctx, serde_json::json!({"query": "  "}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(_)));
}
