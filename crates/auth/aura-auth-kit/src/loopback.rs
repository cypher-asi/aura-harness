//! Ephemeral `127.0.0.1` listener that receives one OAuth callback.

use aura_core_auth::{AuthError, CallbackParams, LoopbackRedirect};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

/// How long a browser sign-in may take before the attempt expires.
pub const CALLBACK_TIMEOUT: Duration = Duration::from_secs(600);

const OK_PAGE: &str = "<!doctype html><title>Aura</title><p>Signed in. Return to Aura to finish connecting; you can close this window.</p>";
const BAD_PAGE: &str = "<!doctype html><title>Aura</title><p>This sign-in callback is invalid or expired. Start again in Aura.</p>";

struct Shared {
    expected_state: String,
    sender: Mutex<Option<oneshot::Sender<CallbackParams>>>,
}

/// A bound callback listener for one authorization attempt.
///
/// Only a request whose `state` matches is accepted; it is delivered
/// once and the server then shuts down. Dropping the listener stops it.
pub struct LoopbackListener {
    redirect_uri: String,
    receiver: oneshot::Receiver<CallbackParams>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl LoopbackListener {
    /// Bind an OS-assigned port on `127.0.0.1` and start serving
    /// `redirect.path`.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Unavailable`] when no local port can be bound.
    pub async fn bind(
        redirect: LoopbackRedirect,
        expected_state: String,
    ) -> Result<Self, AuthError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| {
                AuthError::Unavailable("Could not open the local sign-in callback".into())
            })?;
        let port = listener
            .local_addr()
            .map_err(|_| AuthError::Unavailable("Local sign-in callback unavailable".into()))?
            .port();
        let (sender, receiver) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let shared = Arc::new(Shared {
            expected_state,
            sender: Mutex::new(Some(sender)),
        });
        let app = Router::new()
            .route(redirect.path, get(receive))
            .with_state(shared);
        // Browsers may resolve `localhost` to `::1`; serve it too when free.
        let v6 = if redirect.host == "localhost" {
            tokio::net::TcpListener::bind(("::1", port)).await.ok()
        } else {
            None
        };
        let (v6_shutdown_tx, v6_shutdown_rx) = oneshot::channel::<()>();
        if let Some(v6) = v6 {
            let app = app.clone();
            tokio::spawn(async move {
                let _ = axum::serve(v6, app)
                    .with_graceful_shutdown(async move {
                        let _ = tokio::time::timeout(CALLBACK_TIMEOUT, v6_shutdown_rx).await;
                    })
                    .await;
            });
        }
        tokio::spawn(async move {
            let _v6_shutdown = v6_shutdown_tx;
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = tokio::time::timeout(CALLBACK_TIMEOUT, shutdown_rx).await;
                })
                .await;
        });
        Ok(Self {
            redirect_uri: format!("http://{}:{port}{}", redirect.host, redirect.path),
            receiver,
            shutdown: Some(shutdown_tx),
        })
    }

    /// Redirect URI to register on the authorization request.
    #[must_use]
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Wait for the browser callback.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Declined`] when the attempt expires or the
    /// listener stops before a valid callback arrives.
    pub async fn wait(mut self, timeout: Duration) -> Result<CallbackParams, AuthError> {
        let receiver = &mut self.receiver;
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(params)) => Ok(params),
            Ok(Err(_)) => Err(AuthError::Declined("Sign-in callback closed".into())),
            Err(_) => Err(AuthError::Declined(
                "Sign-in expired before the browser returned; start again".into(),
            )),
        }
    }
}

impl Drop for LoopbackListener {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

async fn receive(
    State(shared): State<Arc<Shared>>,
    Query(params): Query<CallbackParams>,
) -> (StatusCode, Html<&'static str>) {
    if params.state != shared.expected_state {
        return (StatusCode::BAD_REQUEST, Html(BAD_PAGE));
    }
    let sender = shared.sender.lock().ok().and_then(|mut s| s.take());
    match sender.map(|s| s.send(params)) {
        Some(Ok(())) => (StatusCode::OK, Html(OK_PAGE)),
        _ => (StatusCode::BAD_REQUEST, Html(BAD_PAGE)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REDIRECT: LoopbackRedirect = LoopbackRedirect {
        host: "127.0.0.1",
        path: "/callback",
    };

    async fn get_status(url: &str) -> u16 {
        reqwest::get(url).await.unwrap().status().as_u16()
    }

    #[tokio::test]
    async fn accepts_matching_state_once() {
        let listener = LoopbackListener::bind(REDIRECT, "good".into())
            .await
            .unwrap();
        let base = listener.redirect_uri().to_string();
        assert!(base.starts_with("http://127.0.0.1:"));
        assert_eq!(get_status(&format!("{base}?state=bad&code=x")).await, 400);
        assert_eq!(
            get_status(&format!("{base}?state=good&code=abc")).await,
            200
        );
        assert_eq!(
            get_status(&format!("{base}?state=good&code=again")).await,
            400
        );
        let params = listener.wait(Duration::from_secs(5)).await.unwrap();
        assert_eq!(params.code.as_deref(), Some("abc"));
    }

    #[tokio::test]
    async fn expires_without_callback() {
        let listener = LoopbackListener::bind(REDIRECT, "s".into()).await.unwrap();
        let err = listener.wait(Duration::from_millis(50)).await.unwrap_err();
        assert!(matches!(err, AuthError::Declined(_)));
    }

    #[tokio::test]
    async fn localhost_host_is_written_into_redirect() {
        let listener = LoopbackListener::bind(
            LoopbackRedirect {
                host: "localhost",
                path: "/cb",
            },
            "s".into(),
        )
        .await
        .unwrap();
        assert!(listener.redirect_uri().starts_with("http://localhost:"));
        assert!(listener.redirect_uri().ends_with("/cb"));
    }
}
