use std::fmt;

use zeroize::Zeroizing;

use super::{BOXD_API_KEY, BOXD_API_KEY_PREFIX, BOXD_TOKEN};

/// The last four characters of `secret`, or fewer when it is short.
/// Secrets shorter than 12 characters show nothing at all.
fn tail(secret: &str) -> &str {
    let chars = secret.chars().count();
    if chars < 12 {
        return "";
    }
    let skip = secret
        .char_indices()
        .nth(chars - 4)
        .map_or(secret.len(), |(index, _)| index);
    &secret[skip..]
}

/// A boxd API key (`bxd_` + 43 base62 characters). Exchanged for a
/// short-lived token before each hour of calls.
#[derive(Clone, PartialEq, Eq)]
pub struct BoxdApiKey(Zeroizing<String>);

impl BoxdApiKey {
    /// Wrap a raw key.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(Zeroizing::new(raw.into()))
    }

    /// The raw key, for the one call that sends it to boxd.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the key has boxd's `bxd_` prefix.
    #[must_use]
    pub fn looks_valid(&self) -> bool {
        self.0.starts_with(BOXD_API_KEY_PREFIX)
    }
}

impl fmt::Display for BoxdApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{BOXD_API_KEY_PREFIX}…{}", tail(&self.0))
    }
}

impl fmt::Debug for BoxdApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BoxdApiKey({self})")
    }
}

/// A boxd bearer token the user already holds (a session JWT or a
/// machine token, `bxdm_…`).
#[derive(Clone, PartialEq, Eq)]
pub struct BoxdToken(Zeroizing<String>);

impl BoxdToken {
    /// Wrap a raw token.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(Zeroizing::new(raw.into()))
    }

    /// The raw token, for the one call that sends it to boxd.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BoxdToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "…{}", tail(&self.0))
    }
}

impl fmt::Debug for BoxdToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BoxdToken({self})")
    }
}

/// How the app signs in to boxd.
#[derive(Clone, PartialEq, Eq)]
pub enum BoxdCredential {
    /// `BOXD_TOKEN`: used as is until it expires.
    Token(BoxdToken),
    /// `BOXD_API_KEY`: exchanged for a one-hour token, refreshed as needed.
    ApiKey(BoxdApiKey),
}

impl BoxdCredential {
    /// Which variable the credential came from, for status lines.
    #[must_use]
    pub const fn source_var(&self) -> &'static str {
        match self {
            Self::Token(_) => BOXD_TOKEN,
            Self::ApiKey(_) => BOXD_API_KEY,
        }
    }
}

impl fmt::Display for BoxdCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(token) => write!(f, "{BOXD_TOKEN} {token}"),
            Self::ApiKey(key) => write!(f, "{BOXD_API_KEY} {key}"),
        }
    }
}

impl fmt::Debug for BoxdCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(token) => write!(f, "Token({token:?})"),
            Self::ApiKey(key) => write!(f, "ApiKey({key:?})"),
        }
    }
}
