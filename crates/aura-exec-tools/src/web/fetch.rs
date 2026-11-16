//! `web_fetch`: download a page and return it as readable markdown.

use super::html::html_to_markdown;
use super::{fetch_client, read_capped, truncate_chars, validate_url, WebError, REQUEST_TIMEOUT};
use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::{ToolDefinition, ToolResult};
use reqwest::header::CONTENT_TYPE;
use reqwest::Url;
use serde::Deserialize;
use tracing::debug;

/// Tool name used for dispatch.
pub const WEB_FETCH_TOOL_NAME: &str = "web_fetch";

/// Most bytes downloaded from one URL. Longer bodies are cut, not refused:
/// the start of a long page is still useful.
const MAX_DOWNLOAD_BYTES: usize = 2 * 1024 * 1024;

const DEFAULT_MAX_CHARS: usize = 40_000;
const MIN_MAX_CHARS: usize = 1_000;
const MAX_MAX_CHARS: usize = 200_000;

/// Settings for `web_fetch`, read once at startup.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WebFetchSettings {
    /// `AURA_WEB_FETCH_ALLOW_PRIVATE`: allow localhost and private
    /// network addresses (e.g. a local dev server).
    pub allow_private: bool,
}

impl WebFetchSettings {
    /// Read the settings through `get` (normally `std::env::var`).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let allow_private = get("AURA_WEB_FETCH_ALLOW_PRIVATE").is_some_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        });
        Self { allow_private }
    }
}

/// The `web_fetch` tool. Cheap to clone; clones share one HTTP client.
#[derive(Clone)]
pub struct WebFetchTool {
    client: reqwest::Client,
    allow_private: bool,
}

impl WebFetchTool {
    /// # Errors
    /// Fails when the HTTP client cannot be created (TLS backend setup).
    pub fn new(settings: WebFetchSettings) -> Result<Self, ToolError> {
        let client = fetch_client(settings.allow_private)
            .map_err(|e| ToolError::ExternalToolError(e.to_string()))?;
        Ok(Self {
            client,
            allow_private: settings.allow_private,
        })
    }

    async fn fetch(&self, args: &FetchArgs) -> Result<String, WebError> {
        let url = validate_url(&args.url, self.allow_private)?;
        let shown = url.to_string();
        let response = self
            .client
            .get(url)
            .header(
                reqwest::header::ACCEPT,
                "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.5",
            )
            .send()
            .await
            .map_err(|e| WebError::from_reqwest(&shown, &e))?;
        let final_url = response.url().clone();
        let status = response.status();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let kind = classify(content_type.as_deref());
        if let Kind::Binary(ct) = &kind {
            return Err(WebError::NotText {
                url: final_url.to_string(),
                content_type: ct.clone(),
            });
        }

        let (body, cut) = read_capped(final_url.as_str(), response, MAX_DOWNLOAD_BYTES).await?;
        let text = decode(&body, content_type.as_deref());
        let is_html = match kind {
            Kind::Html => true,
            Kind::Text => false,
            Kind::Unknown => {
                if body.iter().take(1024).any(|b| *b == 0) {
                    return Err(WebError::NotText {
                        url: final_url.to_string(),
                        content_type: "binary data (no Content-Type)".into(),
                    });
                }
                looks_like_html(&text)
            }
            Kind::Binary(_) => false,
        };
        let (title, content) = if is_html {
            let page = html_to_markdown(&text, Some(&final_url));
            (page.title, page.markdown)
        } else {
            (None, text)
        };

        if !status.is_success() {
            let excerpt = truncate_chars(content.trim(), 1_000);
            let detail = if excerpt.is_empty() {
                String::new()
            } else {
                format!(":\n\n{excerpt}")
            };
            return Err(WebError::Status {
                url: final_url.to_string(),
                status: status.as_u16(),
                detail,
            });
        }
        debug!(url = %final_url, bytes = body.len(), cut, "web_fetch read page");
        Ok(render_output(
            &final_url,
            title.as_deref(),
            content_type.as_deref(),
            &content,
            args,
            cut,
        ))
    }
}

#[derive(Debug, Deserialize)]
struct FetchArgs {
    url: String,
    #[serde(default)]
    max_chars: Option<usize>,
    #[serde(default)]
    start_index: Option<usize>,
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Html,
    Text,
    Unknown,
    Binary(String),
}

fn classify(content_type: Option<&str>) -> Kind {
    let Some(raw) = content_type else {
        return Kind::Unknown;
    };
    let mime = raw
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if mime.is_empty() {
        return Kind::Unknown;
    }
    if mime == "text/html" || mime == "application/xhtml+xml" {
        return Kind::Html;
    }
    let textual_app = [
        "application/json",
        "application/xml",
        "application/javascript",
        "application/x-javascript",
        "application/ecmascript",
        "application/yaml",
        "application/x-yaml",
        "application/toml",
        "application/x-sh",
        "application/sql",
        "application/graphql",
    ];
    if mime.starts_with("text/")
        || textual_app.contains(&mime.as_str())
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
    {
        return Kind::Text;
    }
    Kind::Binary(mime)
}

fn looks_like_html(text: &str) -> bool {
    let head: String = text
        .trim_start()
        .chars()
        .take(512)
        .collect::<String>()
        .to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html") || head.contains("<body")
}

/// Decode the body as UTF-8, or as Latin-1 when the server says so.
fn decode(body: &[u8], content_type: Option<&str>) -> String {
    let charset = content_type
        .and_then(|ct| {
            ct.split(';')
                .filter_map(|p| p.trim().split_once('='))
                .find(|(k, _)| k.trim().eq_ignore_ascii_case("charset"))
                .map(|(_, v)| v.trim().trim_matches('"').to_ascii_lowercase())
        })
        .unwrap_or_default();
    if matches!(
        charset.as_str(),
        "iso-8859-1" | "latin1" | "latin-1" | "windows-1252" | "us-ascii"
    ) {
        body.iter().map(|b| char::from(*b)).collect()
    } else {
        String::from_utf8_lossy(body).into_owned()
    }
}

fn render_output(
    url: &Url,
    title: Option<&str>,
    content_type: Option<&str>,
    content: &str,
    args: &FetchArgs,
    download_cut: bool,
) -> String {
    let max_chars = args
        .max_chars
        .unwrap_or(DEFAULT_MAX_CHARS)
        .clamp(MIN_MAX_CHARS, MAX_MAX_CHARS);
    let start = args.start_index.unwrap_or(0);
    let total = content.chars().count();

    let mut out = format!("URL: {url}\n");
    if let Some(title) = title {
        out.push_str(&format!("Title: {title}\n"));
    }
    if let Some(ct) = content_type {
        out.push_str(&format!("Content-Type: {ct}\n"));
    }
    out.push('\n');

    if start >= total && total > 0 {
        out.push_str(&format!(
            "[start_index {start} is past the end; the page has {total} characters]"
        ));
        return out;
    }
    let rest = content
        .char_indices()
        .nth(start)
        .map_or("", |(idx, _)| &content[idx..]);
    let shown = truncate_chars(rest, max_chars);
    if shown.trim().is_empty() {
        out.push_str("[the page has no readable text]");
    } else {
        out.push_str(shown);
    }
    let end = start + shown.chars().count();
    if end < total {
        out.push_str(&format!(
            "\n\n[showing characters {start}..{end} of {total}; call web_fetch again with \
             start_index={end} to read on]"
        ));
    }
    if download_cut {
        out.push_str(&format!(
            "\n\n[the page is larger than {} MiB; only its start was read]",
            MAX_DOWNLOAD_BYTES / (1024 * 1024)
        ));
    }
    out
}

/// Definition sent to the model.
#[must_use]
pub fn web_fetch_definition() -> ToolDefinition {
    ToolDefinition {
        name: WEB_FETCH_TOOL_NAME.into(),
        description: format!(
            "Fetch a public web page (http or https) and return its main content as markdown: \
             headings, links, lists and code blocks are kept, scripts, styles and navigation \
             are dropped. Plain text and JSON are returned as-is; images, PDFs and other binary \
             files are refused. Long pages are cut at max_chars; use start_index to read further. \
             Requests time out after {}s.",
            REQUEST_TIMEOUT.as_secs()
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "Absolute http(s) URL to fetch."},
                "max_chars": {
                    "type": "integer",
                    "description": format!("Most characters to return (default {DEFAULT_MAX_CHARS}, max {MAX_MAX_CHARS}).")
                },
                "start_index": {
                    "type": "integer",
                    "description": "Character offset to start from, for reading a long page in parts (default 0)."
                }
            },
            "required": ["url"]
        }),
        cache_control: None,
        eager_input_streaming: None,
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        WEB_FETCH_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        web_fetch_definition()
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let args: FetchArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("web_fetch: {e}")))?;
        match self.fetch(&args).await {
            Ok(text) => Ok(ToolResult::success(WEB_FETCH_TOOL_NAME, text)),
            Err(e) => Ok(ToolResult::failure(WEB_FETCH_TOOL_NAME, e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
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
}
