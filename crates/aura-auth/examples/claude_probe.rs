//! Temporary diagnostic: send minimal Claude-plan requests in a few shapes
//! and print status + rate-limit headers. Never prints the token.

use aura_auth::AuthManager;
use aura_core_auth::{CredentialResolver, ProviderKind};
use serde_json::json;

const IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

#[tokio::main]
async fn main() {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "claude-opus-5-5".into());
    let auth = AuthManager::new().expect("auth manager");
    let cred = auth
        .inference_credential(ProviderKind::Claude)
        .await
        .expect("claude credential");
    let client = reqwest::Client::new();

    let joined = json!(format!("{IDENTITY}\n\nBe brief."));
    let blocks = json!([
        {"type": "text", "text": IDENTITY},
        {"type": "text", "text": "Be brief."}
    ]);
    let variants = [
        ("joined system string, aura headers", joined, false),
        ("identity as its own system block, aura headers", blocks.clone(), false),
        ("identity block + claude-code headers", blocks, true),
    ];
    for (name, system, cc_headers) in variants {
        let body = json!({
            "model": model,
            "max_tokens": 16,
            "system": system,
            "messages": [{"role": "user", "content": "hi"}],
        });
        let mut req = client
            .post("https://api.anthropic.com/v1/messages")
            .bearer_auth(&cred.bearer)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body).unwrap());
        let mut beta: Vec<String> = cred
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("anthropic-beta"))
            .map(|(_, v)| v.clone())
            .collect();
        if cc_headers {
            beta.insert(0, "claude-code-20250219".into());
            req = req
                .header("user-agent", "claude-cli/2.1.2 (external, cli)")
                .header("x-app", "cli");
        }
        req = req.header("anthropic-beta", beta.join(","));
        let resp = req.send().await.expect("send");
        println!("== {name}: {}", resp.status());
        for (k, v) in resp.headers() {
            let k = k.as_str();
            if k.contains("ratelimit") || k == "retry-after" || k == "x-should-retry" {
                println!("   {k}: {}", v.to_str().unwrap_or("?"));
            }
        }
        let text = resp.text().await.unwrap_or_default();
        println!("   body: {}", text.chars().take(300).collect::<String>());
    }
}
