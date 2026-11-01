//! id_token verification against the issuer's JWKS.

use aura_core_auth::AuthError;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Deserialize)]
pub(crate) struct Claims {
    pub sub: String,
    #[serde(default)]
    pub nonce: String,
    #[serde(default)]
    pub azp: Option<String>,
    pub aud: Value,
    #[serde(default)]
    pub email: String,
}

fn invalid() -> AuthError {
    AuthError::Invalid("ChatGPT identity could not be verified".into())
}

/// Verify `token`'s signature, issuer, audience and (when given) nonce.
pub(crate) fn verify(
    token: &str,
    keys: &JwkSet,
    issuer: &str,
    client: &str,
    nonce: Option<&str>,
) -> Result<Claims, AuthError> {
    let header = decode_header(token).map_err(|_| invalid())?;
    if ![Algorithm::RS256, Algorithm::ES256].contains(&header.alg) {
        return Err(invalid());
    }
    let key = keys
        .find(header.kid.as_deref().ok_or_else(invalid)?)
        .ok_or_else(invalid)?;
    let mut validation = Validation::new(header.alg);
    validation.set_required_spec_claims(&["iss", "aud", "exp", "sub"]);
    validation.set_audience(&[client]);
    validation.set_issuer(&[issuer]);
    validation.leeway = 0;
    validation.validate_nbf = true;
    let claims = decode::<Claims>(
        token,
        &DecodingKey::from_jwk(key).map_err(|_| invalid())?,
        &validation,
    )
    .map_err(|_| invalid())?
    .claims;
    let multi_audience = claims.aud.as_array().is_some_and(|a| a.len() > 1);
    if nonce.is_some_and(|n| n != claims.nonce)
        || claims.sub.is_empty()
        || claims.azp.as_ref().is_some_and(|a| a != client)
        || (multi_audience && claims.azp.as_deref() != Some(client))
    {
        return Err(invalid());
    }
    Ok(claims)
}
