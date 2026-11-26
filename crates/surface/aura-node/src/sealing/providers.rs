//! DEK sources: the CoCo confidential-data-hub ([`CdhDekProvider`]) and the
//! dev-mode local key file ([`LocalKeyFileDekProvider`]), plus the key-id
//! to KBS resource path mapping and DEK decoding.

use super::DekProvider;
use anyhow::{bail, Context};
use async_trait::async_trait;
use aura_store_db::SealCipher;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::info;
use zeroize::Zeroizing;

/// Map a four-segment key id onto the three-segment KBS/CDH resource path
/// `(repository, type, tag)`. Must mirror `kbs_resource_path` in the
/// aura-swarm control plane exactly.
///
/// # Errors
/// Rejects key ids with fewer than three segments or empty segments.
pub fn kbs_resource_path(key_id: &str) -> anyhow::Result<(String, String, String)> {
    let segments: Vec<&str> = key_id.split('/').collect();
    if segments.len() < 3 || segments.iter().any(|s| s.is_empty()) {
        bail!("invalid KBS key id (expected at least repository/type/tag): {key_id}");
    }
    let repository = segments[0].to_string();
    let tag = segments[segments.len() - 1].to_string();
    let rtype = segments[1..segments.len() - 1].join(".");
    Ok((repository, rtype, tag))
}

/// Fetches the DEK from the in-guest confidential-data-hub, which performs
/// the attestation handshake with the Trustee KBS transparently.
pub struct CdhDekProvider {
    client: reqwest::Client,
    /// Full resource URL, e.g.
    /// `http://127.0.0.1:8006/cdh/resource/swarm/agents.{id}/state-key`.
    resource_url: String,
}

impl CdhDekProvider {
    /// Build a provider for `key_id` against the CDH at `base_url`.
    ///
    /// # Errors
    /// Returns an error for an invalid key id or HTTP client failure.
    pub fn new(base_url: &str, key_id: &str) -> anyhow::Result<Self> {
        let (repository, rtype, tag) = kbs_resource_path(key_id)?;
        let resource_url = format!(
            "{}/cdh/resource/{repository}/{rtype}/{tag}",
            base_url.trim_end_matches('/')
        );
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .context("building CDH HTTP client")?;
        Ok(Self {
            client,
            resource_url,
        })
    }

    /// The computed resource URL (non-secret; exposed for tests/logs).
    #[must_use]
    pub fn resource_url(&self) -> &str {
        &self.resource_url
    }
}

#[async_trait]
impl DekProvider for CdhDekProvider {
    async fn fetch_dek(&self) -> anyhow::Result<Zeroizing<[u8; 32]>> {
        let response = self
            .client
            .get(&self.resource_url)
            .send()
            .await
            .with_context(|| format!("CDH resource request to {} failed", self.resource_url))?;

        let status = response.status();
        if !status.is_success() {
            // Do not include the body verbatim at error level paths that
            // could ever carry key material; status is enough to diagnose.
            bail!(
                "CDH resource request to {} returned {status}",
                self.resource_url
            );
        }

        let body = Zeroizing::new(
            response
                .bytes()
                .await
                .context("reading CDH response body")?
                .to_vec(),
        );
        decode_dek(&body).with_context(|| {
            format!(
                "CDH resource at {} did not contain a 256-bit DEK",
                self.resource_url
            )
        })
    }

    fn describe(&self) -> String {
        format!("cdh:{}", self.resource_url)
    }
}

/// Decode a DEK from raw bytes (32), hex (64 chars), or base64 text.
///
/// The aura-swarm control plane registers the DEK as raw 32 bytes
/// (`application/octet-stream`); the textual forms are accepted for
/// hand-provisioned dev keys.
pub(super) fn decode_dek(body: &[u8]) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let mut out = Zeroizing::new([0u8; 32]);
    if body.len() == 32 {
        out.copy_from_slice(body);
        return Ok(out);
    }

    let text = std::str::from_utf8(body)
        .map(str::trim)
        .map_err(|_| anyhow::anyhow!("DEK is neither 32 raw bytes nor text-encoded"))?;

    if text.len() == 64 {
        if let Ok(bytes) = hex_decode(text) {
            out.copy_from_slice(&bytes);
            return Ok(out);
        }
    }

    use base64::Engine as _;
    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(text) {
        if bytes.len() == 32 {
            out.copy_from_slice(&bytes);
            return Ok(out);
        }
    }

    bail!("DEK has invalid length/encoding (expected 32 raw bytes, 64 hex chars, or base64)")
}

/// Minimal hex decoder (avoids adding a `hex` dependency to this crate).
fn hex_decode(text: &str) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    fn nibble(c: u8) -> anyhow::Result<u8> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => bail!("invalid hex character"),
        }
    }
    let raw = text.as_bytes();
    if raw.len() % 2 != 0 {
        bail!("odd-length hex string");
    }
    let mut out = Zeroizing::new(Vec::with_capacity(raw.len() / 2));
    for pair in raw.chunks_exact(2) {
        out.push((nibble(pair[0])? << 4) | nibble(pair[1])?);
    }
    Ok(out)
}

/// Dev-mode DEK source: a local key file (64 hex chars), generated with a
/// fresh random 256-bit key on first boot (mode `0600` on Unix). Uses the
/// same sealed on-disk format as production so dev and prod state are
/// interchangeable given the key.
pub struct LocalKeyFileDekProvider {
    path: PathBuf,
}

impl LocalKeyFileDekProvider {
    /// Build a provider reading/creating the key file at `path`.
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[async_trait]
impl DekProvider for LocalKeyFileDekProvider {
    async fn fetch_dek(&self) -> anyhow::Result<Zeroizing<[u8; 32]>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let bytes = Zeroizing::new(bytes);
                decode_dek(&bytes)
                    .with_context(|| format!("invalid key file {}", self.path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = SealCipher::generate_key();
                let encoded = Zeroizing::new(to_hex(&key));
                if let Some(parent) = self.path.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent).with_context(|| {
                            format!("creating key file directory {}", parent.display())
                        })?;
                    }
                }
                write_key_file(&self.path, encoded.as_bytes())
                    .with_context(|| format!("writing key file {}", self.path.display()))?;
                info!(path = %self.path.display(), "generated new dev-mode state key file");
                Ok(key)
            }
            Err(e) => {
                Err(anyhow::Error::new(e)
                    .context(format!("reading key file {}", self.path.display())))
            }
        }
    }

    fn describe(&self) -> String {
        format!("key-file:{}", self.path.display())
    }
}

fn to_hex(bytes: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Persist the dev key file with tight permissions (`0600` on Unix; on
/// Windows NTFS ACLs are inherited from the parent directory, matching
/// how the node's `auth_token` file is handled).
fn write_key_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    #[cfg(unix)]
    let mut f = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?
    };
    #[cfg(not(unix))]
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(contents)?;
    f.sync_all()
}
