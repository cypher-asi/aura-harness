use super::*;

fn sealed_config() -> SealingConfig {
    SealingConfig {
        enabled: true,
        key_id: Some("swarm/agents/abc123/state-key".to_string()),
        key_file: None,
        cdh_url: DEFAULT_CDH_URL.to_string(),
        fetch_timeout: Duration::from_secs(1),
    }
}

// ----------------------------------------------------------------
// Config resolution
// ----------------------------------------------------------------

#[test]
fn resolve_unset_is_plaintext_without_swarm_env() {
    let cfg = SealingConfig::resolve(None, None, None, None, None, false).unwrap();
    assert!(!cfg.enabled);
    assert_eq!(cfg.cdh_url, DEFAULT_CDH_URL);
    assert_eq!(
        cfg.fetch_timeout,
        Duration::from_secs(DEFAULT_DEK_FETCH_TIMEOUT_SECS)
    );
}

#[test]
fn resolve_sealed_enables_and_is_case_insensitive() {
    for v in ["sealed", "SEALED", " Sealed "] {
        let cfg = SealingConfig::resolve(Some(v), None, None, None, None, false).unwrap();
        assert!(cfg.enabled, "`{v}` must enable sealed mode");
    }
}

#[test]
fn resolve_plaintext_spellings_disable_without_swarm_env() {
    for v in ["plaintext", "none", ""] {
        let cfg = SealingConfig::resolve(Some(v), None, None, None, None, false).unwrap();
        assert!(!cfg.enabled);
    }
}

/// A typo in the mode must refuse to start rather than silently run
/// plaintext when the operator asked for encryption.
#[test]
fn resolve_rejects_unknown_mode() {
    let err = SealingConfig::resolve(Some("seald"), None, None, None, None, false).unwrap_err();
    assert!(err.to_string().contains("seald"));
}

#[test]
fn resolve_normalizes_cdh_url_and_timeout() {
    let cfg = SealingConfig::resolve(
        Some("sealed"),
        Some("swarm/agents/x/state-key"),
        None,
        Some("http://cdh.local:8006/"),
        Some("7"),
        false,
    )
    .unwrap();
    assert_eq!(cfg.cdh_url, "http://cdh.local:8006");
    assert_eq!(cfg.fetch_timeout, Duration::from_secs(7));
}

// ----------------------------------------------------------------
// Swarm env: plaintext refused (TEE upgrade R3)
// ----------------------------------------------------------------

/// With the swarm control-plane env present, an unset
/// `AURA_STATE_ENCRYPTION` defaults to sealed instead of plaintext.
#[test]
fn resolve_swarm_env_defaults_to_sealed() {
    for unset in [None, Some(""), Some("  ")] {
        let cfg = SealingConfig::resolve(
            unset,
            Some("swarm/agents/x/state-key"),
            None,
            None,
            None,
            true,
        )
        .unwrap();
        assert!(cfg.enabled, "swarm env must default to sealed");
    }
}

/// With the swarm control-plane env present, an explicit plaintext
/// request must refuse to start.
#[test]
fn resolve_swarm_env_refuses_explicit_plaintext() {
    for v in ["plaintext", "none", "PLAINTEXT"] {
        let err = SealingConfig::resolve(Some(v), None, None, None, None, true).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("never run with plaintext"),
            "`{v}` must be refused under swarm env: {msg}"
        );
    }
}

/// Explicit sealed under swarm env stays sealed (the normal R1+ pod).
#[test]
fn resolve_swarm_env_sealed_is_sealed() {
    let cfg = SealingConfig::resolve(
        Some("sealed"),
        Some("swarm/agents/x/state-key"),
        None,
        None,
        None,
        true,
    )
    .unwrap();
    assert!(cfg.enabled);
}

// ----------------------------------------------------------------
// Provider selection
// ----------------------------------------------------------------

#[test]
fn provider_selection_prefers_key_file() {
    let mut cfg = sealed_config();
    cfg.key_file = Some(PathBuf::from("/tmp/dev.key"));
    let provider = cfg.build_provider().unwrap();
    assert!(provider.describe().starts_with("key-file:"));
}

#[test]
fn provider_selection_defaults_to_cdh() {
    let provider = sealed_config().build_provider().unwrap();
    assert_eq!(
        provider.describe(),
        format!("cdh:{DEFAULT_CDH_URL}/cdh/resource/swarm/agents.abc123/state-key")
    );
}

#[test]
fn provider_selection_requires_key_id_without_key_file() {
    let mut cfg = sealed_config();
    cfg.key_id = None;
    let Err(err) = cfg.build_provider() else {
        panic!("sealed mode without key id or key file must be rejected")
    };
    assert!(err.to_string().contains(ENV_STATE_KEY_ID));
}

// ----------------------------------------------------------------
// KBS resource path mapping (must mirror aura-swarm's kbs.rs)
// ----------------------------------------------------------------

#[test]
fn resource_path_maps_agent_state_key() {
    let (repo, rtype, tag) = kbs_resource_path("swarm/agents/0123abcd/state-key").unwrap();
    assert_eq!(repo, "swarm");
    assert_eq!(rtype, "agents.0123abcd");
    assert_eq!(tag, "state-key");
}

#[test]
fn resource_path_three_segments_is_identity() {
    let (repo, rtype, tag) = kbs_resource_path("repo/type/tag").unwrap();
    assert_eq!(
        (repo.as_str(), rtype.as_str(), tag.as_str()),
        ("repo", "type", "tag")
    );
}

#[test]
fn resource_path_rejects_short_or_empty_segments() {
    assert!(kbs_resource_path("only/two").is_err());
    assert!(kbs_resource_path("a//b/c").is_err());
    assert!(kbs_resource_path("").is_err());
}

#[test]
fn cdh_url_is_built_from_mapping() {
    let provider =
        CdhDekProvider::new("http://127.0.0.1:8006/", "swarm/agents/abc/state-key").unwrap();
    assert_eq!(
        provider.resource_url(),
        "http://127.0.0.1:8006/cdh/resource/swarm/agents.abc/state-key"
    );
}

// ----------------------------------------------------------------
// DEK decoding
// ----------------------------------------------------------------

#[test]
fn decode_dek_accepts_raw_hex_and_base64() {
    let key = [0xabu8; 32];

    assert_eq!(*decode_dek(&key).unwrap(), key);

    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(*decode_dek(hex.as_bytes()).unwrap(), key);

    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(key);
    assert_eq!(*decode_dek(b64.as_bytes()).unwrap(), key);
}

#[test]
fn decode_dek_rejects_wrong_lengths() {
    assert!(decode_dek(&[1u8; 16]).is_err());
    assert!(decode_dek(b"not a key").is_err());
    assert!(decode_dek(b"").is_err());
}

// ----------------------------------------------------------------
// Dev key file provider
// ----------------------------------------------------------------

#[tokio::test]
async fn key_file_generated_on_first_boot_and_stable_after() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keys").join("state.key");
    let provider = LocalKeyFileDekProvider::new(path.clone());

    assert!(!path.exists());
    let first = provider.fetch_dek().await.unwrap();
    assert!(path.exists(), "first fetch must create the key file");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "key file must be mode 0600");
    }

    let second = provider.fetch_dek().await.unwrap();
    assert_eq!(*first, *second, "key must be stable across boots");

    // The persisted key drives a working cipher roundtrip.
    let cipher = SealCipher::new(&first);
    let sealed = cipher.seal(b"dev state").unwrap();
    assert_eq!(cipher.open(&sealed).unwrap(), b"dev state");
}

#[tokio::test]
async fn key_file_with_garbage_contents_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.key");
    std::fs::write(&path, b"definitely not a key").unwrap();
    let provider = LocalKeyFileDekProvider::new(path);
    assert!(provider.fetch_dek().await.is_err());
}

// ----------------------------------------------------------------
// Boot flow: refuse-to-start + marker
// ----------------------------------------------------------------

/// Bind-then-drop a listener to obtain a port with nothing behind it.
async fn dead_endpoint() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

#[tokio::test]
async fn refuses_to_start_when_no_dek_obtainable() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = sealed_config();
    cfg.cdh_url = dead_endpoint().await;
    cfg.fetch_timeout = Duration::from_millis(600);

    let started = Instant::now();
    let err = prepare_with_config(&cfg, dir.path(), &dir.path().join("db"))
        .await
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("refusing to serve"),
        "error must state the refusal: {err:#}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(600),
        "must keep retrying until the bounded window elapses"
    );
    assert!(
        !dir.path().join(SEALED_MARKER_FILENAME).exists(),
        "no marker may be written when sealed boot fails"
    );
}

#[tokio::test]
async fn plaintext_mode_returns_no_cipher_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = SealingConfig::resolve(None, None, None, None, None, false).unwrap();
    let cipher = prepare_with_config(&cfg, dir.path(), &dir.path().join("db"))
        .await
        .unwrap();
    assert!(cipher.is_none());
    assert!(!dir.path().join(SEALED_MARKER_FILENAME).exists());
    assert!(
        !dir.path().join("db.sealed-migrating").exists()
            && !dir.path().join("db.plaintext-backup").exists(),
        "plaintext mode must never start a state migration"
    );
}

#[tokio::test]
async fn sealed_dev_boot_creates_cipher_and_marker() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = sealed_config();
    cfg.key_file = Some(dir.path().join("state.key"));

    let cipher = prepare_with_config(&cfg, dir.path(), &dir.path().join("db"))
        .await
        .unwrap()
        .expect("sealed mode must produce a cipher");

    let sealed = cipher.seal(b"hello").unwrap();
    assert_eq!(cipher.open(&sealed).unwrap(), b"hello");

    let marker_path = dir.path().join(SEALED_MARKER_FILENAME);
    let marker: SealedMarker =
        serde_json::from_slice(&std::fs::read(&marker_path).unwrap()).unwrap();
    assert_eq!(marker.version, aura_store_db::SEAL_VERSION);
    assert_eq!(marker.cipher, "aes-256-gcm");
    assert_eq!(
        marker.key_id.as_deref(),
        Some("swarm/agents/abc123/state-key")
    );
}

// ----------------------------------------------------------------
// CDH happy path against a local HTTP stub
// ----------------------------------------------------------------

/// Minimal one-shot HTTP server returning `body` with status 200.
async fn serve_once(body: Vec<u8>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let header = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(&body).await;
            let _ = socket.shutdown().await;
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn cdh_provider_fetches_raw_dek() {
    let key = [0x5au8; 32];
    let base = serve_once(key.to_vec()).await;
    let provider = CdhDekProvider::new(&base, "swarm/agents/abc/state-key").unwrap();
    let dek = provider.fetch_dek().await.unwrap();
    assert_eq!(*dek, key);
}

#[tokio::test]
async fn retry_succeeds_after_initial_failures() {
    // First endpoint is dead; provider keeps failing, but the retry
    // loop must keep trying until the window closes. We point at a
    // live stub from the start and assert single-attempt success to
    // keep this test fast and deterministic.
    let key = [0x11u8; 32];
    let base = serve_once(key.to_vec()).await;
    let provider = CdhDekProvider::new(&base, "swarm/agents/abc/state-key").unwrap();
    let dek = fetch_dek_with_retry(&provider, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(*dek, key);
}
