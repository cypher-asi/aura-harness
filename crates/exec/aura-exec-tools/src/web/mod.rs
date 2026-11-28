//! Web access for agents: `web_fetch` (a page as markdown) and
//! `web_search` (a configured search provider).
//!
//! Both are ordinary executor tools, so every call goes through the
//! kernel's policy check and is recorded like any other tool call. Every
//! request has a connect and a total timeout, and responses are read up to
//! a byte cap.

pub mod fetch;
pub mod html;
pub mod search;

pub use fetch::{web_fetch_definition, WebFetchSettings, WebFetchTool, WEB_FETCH_TOOL_NAME};
pub use search::{
    build_provider, web_search_definition, BraveSearch, ProviderChoice, SearchHit, SearchProvider,
    SearxngSearch, TavilySearch, WebSearchSettings, WebSearchTool, WEB_SEARCH_TOOL_NAME,
};

use aura_core_types::ErrorChain;
use reqwest::Url;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

/// Total time allowed for one request, body included.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Time allowed to open the connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Redirects followed before a fetch gives up.
pub const MAX_REDIRECTS: usize = 5;

const USER_AGENT: &str = concat!("aura-harness/", env!("CARGO_PKG_VERSION"), " (web tools)");

/// Errors from the web tools. They reach the model as tool failures.
#[derive(Debug, thiserror::Error)]
pub enum WebError {
    #[error("invalid URL `{url}`: {reason}")]
    InvalidUrl { url: String, reason: String },
    #[error(
        "`{host}` is a local or private address; web_fetch only reads public sites \
         (set AURA_WEB_FETCH_ALLOW_PRIVATE=1 to allow local addresses)"
    )]
    PrivateAddress { host: String },
    #[error("{url} returned `{content_type}`; web_fetch only reads text pages (HTML, plain text, JSON, XML)")]
    NotText { url: String, content_type: String },
    #[error("{url} returned HTTP {status}{detail}")]
    Status {
        url: String,
        status: u16,
        detail: String,
    },
    #[error("request to {url} timed out after {}s", REQUEST_TIMEOUT.as_secs())]
    Timeout { url: String },
    #[error("request to {url} failed: {reason}")]
    Request { url: String, reason: String },
    #[error("{0}")]
    NotConfigured(String),
    #[error("{provider} returned an unexpected response: {reason}")]
    BadResponse {
        provider: &'static str,
        reason: String,
    },
    #[error("could not create the HTTP client: {0}")]
    Client(String),
}

impl WebError {
    fn from_reqwest(url: &str, err: &reqwest::Error) -> Self {
        if err.is_timeout() {
            return Self::Timeout { url: url.into() };
        }
        // Redirect-policy rejections carry our own message as the source.
        let mut reason = err.to_string();
        let mut source = std::error::Error::source(err);
        while let Some(inner) = source {
            reason = inner.to_string();
            source = inner.source();
        }
        Self::Request {
            url: url.into(),
            reason,
        }
    }
}

/// Parse `raw` and check it is something the web tools may request: an
/// absolute `http`/`https` URL with a host and no embedded credentials,
/// and, unless `allow_private`, not a loopback, private, or link-local
/// address.
///
/// Only literal addresses and `localhost` names are recognised; a public
/// name that resolves to a private address is not caught here.
pub fn validate_url(raw: &str, allow_private: bool) -> Result<Url, WebError> {
    let invalid = |reason: &str| WebError::InvalidUrl {
        url: raw.to_string(),
        reason: reason.to_string(),
    };
    let url = Url::parse(raw.trim()).map_err(|e| invalid(&e.to_string()))?;
    check_url(&url, allow_private).map_err(|e| match e {
        WebError::InvalidUrl { reason, .. } => invalid(&reason),
        other => other,
    })?;
    Ok(url)
}

fn check_url(url: &Url, allow_private: bool) -> Result<(), WebError> {
    let invalid = |reason: &str| WebError::InvalidUrl {
        url: url.to_string(),
        reason: reason.to_string(),
    };
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid("only http and https URLs can be fetched"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("URLs with embedded credentials are not allowed"));
    }
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| invalid("the URL has no host"))?;
    if allow_private {
        return Ok(());
    }
    // `Url` normalises hosts: IPv4 is dotted-decimal, IPv6 is bracketed.
    let private = match host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
    {
        Ok(ip) => is_private_ip(ip),
        Err(_) => {
            let name = host.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
    };
    if private {
        return Err(WebError::PrivateAddress {
            host: host.to_string(),
        });
    }
    Ok(())
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_v4(v4);
            }
            let first = v6.segments()[0];
            v6 == Ipv6Addr::LOCALHOST
                || v6 == Ipv6Addr::UNSPECIFIED
                || (first & 0xfe00) == 0xfc00 // unique local
                || (first & 0xffc0) == 0xfe80 // link local
        }
    }
}

fn is_private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        // Carrier-grade NAT, 100.64.0.0/10.
        || (a == 100 && (64..128).contains(&b))
}

/// Client for `web_fetch`: bounded timeouts and a redirect policy that
/// re-checks every hop, so a public page cannot bounce the fetch to a
/// local address or a non-HTTP scheme.
pub(crate) fn fetch_client(allow_private: bool) -> Result<reqwest::Client, WebError> {
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.error(format!("stopped after {MAX_REDIRECTS} redirects"));
        }
        match check_url(attempt.url(), allow_private) {
            Ok(()) => attempt.follow(),
            Err(e) => attempt.error(format!("redirect refused: {e}")),
        }
    });
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(policy)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| WebError::Client(ErrorChain(&e).to_string()))
}

/// Client for search APIs.
pub(crate) fn api_client() -> Result<reqwest::Client, WebError> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| WebError::Client(ErrorChain(&e).to_string()))
}

/// Read at most `cap` bytes of the body. Returns the bytes and whether
/// the body was longer.
pub(crate) async fn read_capped(
    url: &str,
    mut response: reqwest::Response,
    cap: usize,
) -> Result<(Vec<u8>, bool), WebError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| WebError::from_reqwest(url, &e))?
    {
        let room = cap - body.len();
        if chunk.len() >= room {
            body.extend_from_slice(&chunk[..room]);
            let more = chunk.len() > room || response.chunk().await.ok().flatten().is_some();
            return Ok((body, more));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, false))
}

/// Cut `text` to at most `max` characters on a character boundary.
pub(crate) fn truncate_chars(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((idx, _)) => &text[..idx],
        None => text,
    }
}

#[cfg(test)]
pub(crate) mod test_server;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_public_http_and_https() {
        assert!(validate_url("https://docs.rs/tokio", false).is_ok());
        assert!(validate_url("  http://example.com/a?b=c  ", false).is_ok());
        assert!(validate_url("http://93.184.216.34/", false).is_ok());
    }

    #[test]
    fn rejects_other_schemes_and_credentials() {
        for url in [
            "file:///etc/passwd",
            "ftp://example.com/x",
            "javascript:alert(1)",
            "data:text/html,hi",
            "example.com",
            "https://user:pw@example.com/",
        ] {
            let err = validate_url(url, true).unwrap_err();
            assert!(matches!(err, WebError::InvalidUrl { .. }), "{url}: {err}");
        }
    }

    #[test]
    fn rejects_private_addresses_unless_allowed() {
        for url in [
            "http://localhost:3000/",
            "http://api.localhost/",
            "http://127.0.0.1/",
            "http://10.1.2.3/",
            "http://192.168.0.10/",
            "http://172.16.5.4/",
            "http://169.254.169.254/latest/meta-data",
            "http://100.64.0.1/",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://[fe80::1]/",
            "http://[::ffff:127.0.0.1]/",
        ] {
            let err = validate_url(url, false).unwrap_err();
            assert!(
                matches!(err, WebError::PrivateAddress { .. }),
                "{url}: {err}"
            );
            assert!(validate_url(url, true).is_ok(), "{url} with allow_private");
        }
    }

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate_chars("héllo", 2), "hé");
        assert_eq!(truncate_chars("hi", 10), "hi");
    }
}
