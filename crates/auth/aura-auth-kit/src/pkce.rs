//! PKCE attempt generation and pasted-code parsing.

use aura_core_auth::{AuthError, AuthorizationRequest, CallbackParams};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

/// 64 hex chars of OS randomness (two v4 UUIDs, 244 random bits).
#[must_use]
pub fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Build a fresh authorization attempt with an S256 PKCE challenge.
#[must_use]
pub fn new_attempt(
    client_id: String,
    redirect_uri: String,
    login_hint: Option<String>,
) -> AuthorizationRequest {
    let code_verifier = random_token();
    let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
    AuthorizationRequest {
        client_id,
        redirect_uri,
        state: random_token(),
        nonce: uuid::Uuid::new_v4().simple().to_string(),
        code_verifier,
        code_challenge,
        login_hint,
    }
}

/// Parse what a user pasted from a manual-code page.
///
/// Accepts `code#state`, a full redirect URL with `code` and `state`
/// query parameters, or a bare code (which is then bound to
/// `expected_state`). A pasted state that differs from
/// `expected_state` is rejected.
///
/// # Errors
///
/// Returns [`AuthError::Invalid`] for empty input, a missing code, or
/// a mismatched state.
pub fn parse_pasted_code(input: &str, expected_state: &str) -> Result<CallbackParams, AuthError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AuthError::Invalid(
            "No authorization code was entered".into(),
        ));
    }
    let (code, state) = if let Ok(url) = url::Url::parse(input) {
        let mut code = None;
        let mut state = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "code" => code = Some(value.into_owned()),
                "state" => state = Some(value.into_owned()),
                _ => {}
            }
        }
        (code, state)
    } else if let Some((code, state)) = input.split_once('#') {
        (Some(code.to_string()), Some(state.to_string()))
    } else {
        (Some(input.to_string()), None)
    };
    let code = code
        .filter(|c| !c.is_empty() && c.len() <= 8192)
        .ok_or_else(|| AuthError::Invalid("The pasted text has no authorization code".into()))?;
    let state = state.unwrap_or_else(|| expected_state.to_string());
    if state != expected_state {
        return Err(AuthError::Invalid(
            "The pasted code belongs to a different sign-in attempt".into(),
        ));
    }
    Ok(CallbackParams {
        code: Some(code),
        state,
        client_id: None,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_has_s256_challenge_and_unique_state() {
        let a = new_attempt("client".into(), "http://127.0.0.1:1/cb".into(), None);
        let b = new_attempt("client".into(), "http://127.0.0.1:1/cb".into(), None);
        assert_ne!(a.state, b.state);
        assert_ne!(a.code_verifier, b.code_verifier);
        assert_eq!(a.code_verifier.len(), 64);
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(a.code_verifier.as_bytes()));
        assert_eq!(a.code_challenge, expected);
    }

    #[test]
    fn pasted_code_forms() {
        let p = parse_pasted_code("abc#st", "st").unwrap();
        assert_eq!(p.code.as_deref(), Some("abc"));
        let p = parse_pasted_code(" https://x.test/cb?code=xyz&state=st ", "st").unwrap();
        assert_eq!(p.code.as_deref(), Some("xyz"));
        let p = parse_pasted_code("bare", "st").unwrap();
        assert_eq!(p.state, "st");
    }

    #[test]
    fn pasted_code_rejects_wrong_state_and_empty() {
        assert!(parse_pasted_code("abc#other", "st").is_err());
        assert!(parse_pasted_code("   ", "st").is_err());
        assert!(parse_pasted_code("#st", "st").is_err());
    }
}
