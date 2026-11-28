//! What went wrong with a turn, as far as getting it going again goes:
//! the model refused the sign-in ([`Trouble::SignIn`]), or it could not
//! be reached for a while ([`Trouble::Transient`]). Anything else is not
//! trouble a pause or a retry fixes, and fails as before.
//!
//! A refused sign-in is a 401 or a 403 that is not Cloudflare's. No
//! credits left (402) is not: signing in again does not fix it.

use aura_agent::{AgentError, AgentLoopResult};

/// Trouble a pause or a retry may get a turn past, and its reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Trouble {
    /// The model's route refused the sign-in behind the request
    SignIn(String),
    /// The model was out of reach: a timeout, overload, an upstream 5xx,
    /// a dropped connection or stream
    Transient(String),
}

impl Trouble {
    /// The trouble of a local turn that ended in `result`, whose events
    /// said the route refused its credentials when `credentials_rejected`.
    pub(crate) fn of_turn(
        result: &Result<AgentLoopResult, AgentError>,
        credentials_rejected: bool,
    ) -> Option<Self> {
        match result {
            Ok(result) if result.insufficient_credits => None,
            Ok(result) => {
                let why = result.llm_error.clone()?;
                if credentials_rejected {
                    Some(Self::SignIn(why))
                } else if result.llm_unavailable {
                    Some(Self::Transient(why))
                } else {
                    None
                }
            }
            Err(AgentError::Reason(error)) if error.is_insufficient_credits() => None,
            Err(AgentError::Reason(error)) if error.rejects_credentials() => {
                Some(Self::SignIn(error.to_string()))
            }
            Err(error) if error.is_unavailable() => Some(Self::Transient(error.to_string())),
            Err(_) => None,
        }
    }

    /// The trouble a cloud turn's failure `message` names, read from its
    /// text: the machine sends only that.
    pub(crate) fn of_message(message: &str) -> Option<Self> {
        let lower = message.to_ascii_lowercase();
        let has = |words: &[&str]| words.iter().any(|word| lower.contains(word));
        if has(&["402", "insufficient credits", "insufficient_credits"]) {
            return None;
        }
        let cloudflare = lower.contains("cloudflare");
        let refused = has(&["401", "unauthorized", "authentication_error", "revoked"])
            || (lower.contains("403") && !cloudflare)
            || (lower.contains("forbidden") && !cloudflare);
        if refused {
            return Some(Self::SignIn(message.to_string()));
        }
        let unreachable = has(&[
            "408",
            "429",
            "529",
            "500",
            "502",
            "503",
            "504",
            "overloaded",
            "rate limit",
            "rate_limit",
            "timed out",
            "timeout",
            "connection",
            "stream",
            "stopped answering",
        ]) || cloudflare;
        unreachable.then(|| Self::Transient(message.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_model::ReasonerError;

    #[test]
    fn a_cloud_turns_failure_is_read_from_its_text() {
        let sign_in = |m: &str| Some(Trouble::SignIn(m.to_string()));
        let transient = |m: &str| Some(Trouble::Transient(m.to_string()));
        let cases = [
            (
                "API error (status 401): OAuth access token has been revoked",
                sign_in("API error (status 401): OAuth access token has been revoked"),
            ),
            (
                "authentication_error: invalid x-api-key",
                sign_in("authentication_error: invalid x-api-key"),
            ),
            (
                "API error (status 403): forbidden",
                sign_in("API error (status 403): forbidden"),
            ),
            (
                "API error (status 408): Request timed out",
                transient("API error (status 408): Request timed out"),
            ),
            (
                "overloaded_error: Overloaded",
                transient("overloaded_error: Overloaded"),
            ),
            (
                "Rate limited: retry after 7 seconds",
                transient("Rate limited: retry after 7 seconds"),
            ),
            (
                "API error (status 503): unavailable",
                transient("API error (status 503): unavailable"),
            ),
            (
                "Cloudflare block (403)",
                transient("Cloudflare block (403)"),
            ),
            (
                "its machine stopped answering before the turn ended",
                transient("its machine stopped answering before the turn ended"),
            ),
            ("API error (status 400): bad request", None),
            ("Insufficient credits: 402", None),
            ("parse error: bad json", None),
            ("prompt is too long", None),
        ];
        for (message, expected) in cases {
            assert_eq!(Trouble::of_message(message), expected, "{message}");
        }
    }

    /// A turn that ran to its end on the model error `error`.
    fn ended_with(error: &str, unavailable: bool) -> AgentLoopResult {
        AgentLoopResult {
            llm_error: Some(error.to_string()),
            llm_unavailable: unavailable,
            ..AgentLoopResult::default()
        }
    }

    #[test]
    fn a_local_turns_trouble_comes_from_its_result() {
        let revoked = "API error (status 401): OAuth access token has been revoked";
        assert_eq!(
            Trouble::of_turn(&Ok(ended_with(revoked, true)), true),
            Some(Trouble::SignIn(revoked.to_string()))
        );
        assert_eq!(
            Trouble::of_turn(&Ok(ended_with("timeout", true)), false),
            Some(Trouble::Transient("timeout".to_string()))
        );
        assert_eq!(
            Trouble::of_turn(&Ok(ended_with("bad request", false)), false),
            None
        );
        let credits = Ok(AgentLoopResult {
            insufficient_credits: true,
            llm_error: Some("402".to_string()),
            ..AgentLoopResult::default()
        });
        assert_eq!(Trouble::of_turn(&credits, true), None);
        assert_eq!(
            Trouble::of_turn(&Ok(AgentLoopResult::default()), false),
            None
        );

        let refused = Err(AgentError::Reason(ReasonerError::Api {
            status: 401,
            message: "revoked".to_string(),
        }));
        assert!(matches!(
            Trouble::of_turn(&refused, false),
            Some(Trouble::SignIn(_))
        ));
        let timed_out = Err(AgentError::Reason(ReasonerError::Timeout));
        assert!(matches!(
            Trouble::of_turn(&timed_out, false),
            Some(Trouble::Transient(_))
        ));
        let parse = Err(AgentError::Reason(ReasonerError::Parse("x".to_string())));
        assert_eq!(Trouble::of_turn(&parse, false), None);
    }
}
