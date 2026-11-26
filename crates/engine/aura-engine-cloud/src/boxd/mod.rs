//! [`VmProvider`] over boxd's public gRPC API (`boxd.api.v1.BoxdApi`).
//!
//! ## Behaviour
//!
//! - Account, org and billing calls go to the home endpoint
//!   (`BOXD_BASE_URL`, default `https://boxd.sh:9443`). Machine and
//!   snapshot calls go to the machine's region endpoint
//!   (`nyc.boxd.sh:9443` for `nyc`), remembered per machine.
//! - Every call carries `authorization: Bearer <jwt>` from the shared
//!   [`TokenSource`] and its own timeout. An `UNAUTHENTICATED` answer
//!   drops the cached token and retries once.
//! - A fresh machine whose size the org quota refuses is retried one
//!   size down ([`MachineSize::smaller`]), with a warning.
//! - `Exec` is a bidirectional stream: the first chunk names the
//!   machine and command, later chunks carry stdin, and the client
//!   closing its side is end-of-input. Dropping the call (a timeout)
//!   makes boxd kill the remote command.

mod provider;
mod streams;
mod wire;

pub use self::wire::{
    create_vm_request, machine_info, pick_org, restore_request, set_secret_request, vm_usage,
};

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aura_config::{CloudConfig, MachineSize};
use parking_lot::Mutex;
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

use self::wire::{fork_request, normalize_url};
use crate::error::{CloudError, CloudResult};
use crate::proto::{self, boxd_api_client::BoxdApiClient};
use crate::provider::{MachineInfo, MachineOrigin, NewMachine, VmProvider};
use crate::token::{exchange_url_for, TokenSource};

/// Limit on ordinary calls.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Limit on creating, forking and snapshotting.
pub const CREATE_TIMEOUT: Duration = Duration::from_secs(180);
/// Limit on connecting to an endpoint.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Endpoint URL for `region` given the home endpoint `base`.
#[must_use]
pub fn region_endpoint(base: &str, region: Option<&str>) -> String {
    let Some(region) = region.filter(|r| !r.is_empty()) else {
        return base.to_string();
    };
    let (scheme, rest) = base
        .split_once("://")
        .map_or(("https", base), |(scheme, rest)| (scheme, rest));
    // Only boxd's own cluster has region subdomains.
    if rest.starts_with("boxd.sh") {
        format!("{scheme}://{region}.{rest}")
    } else {
        base.to_string()
    }
}

/// The boxd gRPC client.
pub struct BoxdProvider {
    tokens: Arc<TokenSource>,
    base_url: String,
    region: Option<String>,
    org: String,
    /// The org secrets are set in: `BOXD_ORG`, or the key's default org.
    secret_org: tokio::sync::OnceCell<String>,
    channels: Mutex<HashMap<String, Channel>>,
    /// Region endpoint of each machine seen, by id and by name.
    machine_endpoints: Mutex<HashMap<String, String>>,
}

impl std::fmt::Debug for BoxdProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoxdProvider")
            .field("base_url", &self.base_url)
            .field("region", &self.region)
            .field("org", &self.org)
            .finish_non_exhaustive()
    }
}

impl BoxdProvider {
    /// A provider for `config`, sharing `tokens`.
    #[must_use]
    pub fn new(config: &CloudConfig, tokens: Arc<TokenSource>) -> Self {
        Self {
            tokens,
            base_url: normalize_url(config.base_url()),
            region: config.region.clone(),
            org: config.org.clone().unwrap_or_default(),
            secret_org: tokio::sync::OnceCell::new(),
            channels: Mutex::new(HashMap::new()),
            machine_endpoints: Mutex::new(HashMap::new()),
        }
    }

    /// A provider and its token source from `config`.
    ///
    /// # Errors
    ///
    /// [`CloudError::NotConfigured`] without a credential.
    pub fn from_config(config: &CloudConfig) -> CloudResult<Self> {
        let credential = config.credential.clone().ok_or(CloudError::NotConfigured)?;
        let tokens = Arc::new(TokenSource::new(
            credential,
            exchange_url_for(config.base_url()),
        )?);
        Ok(Self::new(config, tokens))
    }

    /// The shared token source.
    #[must_use]
    pub fn tokens(&self) -> Arc<TokenSource> {
        Arc::clone(&self.tokens)
    }

    fn home(&self) -> String {
        self.base_url.clone()
    }

    /// The organizations the key can act in, as `(id, slug, name)`, and
    /// the caller's default org id.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub async fn orgs(&self) -> CloudResult<(Vec<(String, String, String)>, String)> {
        let answer = self
            .unary(
                "ListOrgs",
                &self.home(),
                proto::ListOrgsRequest {},
                CALL_TIMEOUT,
                |mut c, r| async move { c.list_orgs(r).await },
            )
            .await?;
        Ok((
            answer
                .orgs
                .into_iter()
                .map(|org| (org.id, org.slug, org.name))
                .collect(),
            answer.default_org_id,
        ))
    }

    /// The org to keep secrets in: `BOXD_ORG`, or the org boxd names as
    /// the caller's default (the one an org-fenced key creates in).
    async fn secret_org(&self) -> CloudResult<String> {
        self.secret_org
            .get_or_try_init(|| async {
                if !self.org.is_empty() {
                    return Ok(self.org.clone());
                }
                let answer = self
                    .unary(
                        "ListOrgs",
                        &self.home(),
                        proto::ListOrgsRequest {},
                        CALL_TIMEOUT,
                        |mut c, r| async move { c.list_orgs(r).await },
                    )
                    .await?;
                pick_org(&answer).ok_or(CloudError::InvalidResponse {
                    op: "ListOrgs",
                    message: "no organization to keep secrets in; set BOXD_ORG".into(),
                })
            })
            .await
            .cloned()
    }

    fn create_endpoint(&self) -> String {
        region_endpoint(&self.base_url, self.region.as_deref())
    }

    fn endpoint_of(&self, machine: &str) -> String {
        self.machine_endpoints
            .lock()
            .get(machine)
            .cloned()
            .unwrap_or_else(|| self.create_endpoint())
    }

    fn remember(&self, info: &MachineInfo) {
        let endpoint = region_endpoint(&self.base_url, Some(info.region.as_str()));
        let mut map = self.machine_endpoints.lock();
        map.insert(info.id.clone(), endpoint.clone());
        map.insert(info.name.clone(), endpoint);
    }

    fn channel(&self, endpoint: &str) -> CloudResult<Channel> {
        if let Some(channel) = self.channels.lock().get(endpoint) {
            return Ok(channel.clone());
        }
        let transport = |message: String| CloudError::Transport {
            endpoint: endpoint.to_string(),
            message,
        };
        let mut builder = Endpoint::from_shared(endpoint.to_string())
            .map_err(|err| transport(err.to_string()))?
            .connect_timeout(CONNECT_TIMEOUT)
            .http2_keep_alive_interval(Duration::from_secs(30))
            .keep_alive_while_idle(true);
        if endpoint.starts_with("https://") {
            builder = builder
                .tls_config(ClientTlsConfig::new().with_webpki_roots())
                .map_err(|err| transport(err.to_string()))?;
        }
        let channel = builder.connect_lazy();
        self.channels
            .lock()
            .insert(endpoint.to_string(), channel.clone());
        Ok(channel)
    }

    async fn request<T>(&self, message: T, timeout: Duration) -> CloudResult<tonic::Request<T>> {
        let token = self.tokens.bearer().await?;
        let value = MetadataValue::try_from(format!("Bearer {}", token.expose()))
            .map_err(|_| CloudError::Auth("token is not valid header text".into()))?;
        let mut request = tonic::Request::new(message);
        request.metadata_mut().insert("authorization", value);
        request.set_timeout(timeout);
        Ok(request)
    }

    /// Run one unary call on `endpoint`, retrying once after an
    /// `UNAUTHENTICATED` answer with a fresh token.
    async fn unary<Req, Resp, F, Fut>(
        &self,
        op: &'static str,
        endpoint: &str,
        message: Req,
        timeout: Duration,
        call: F,
    ) -> CloudResult<Resp>
    where
        Req: Clone + Send,
        F: Fn(BoxdApiClient<Channel>, tonic::Request<Req>) -> Fut,
        Fut: std::future::Future<Output = Result<tonic::Response<Resp>, tonic::Status>>,
    {
        let channel = self.channel(endpoint)?;
        for attempt in 0..2 {
            let request = self.request(message.clone(), timeout).await?;
            let outcome = tokio::time::timeout(
                timeout + Duration::from_secs(1),
                call(BoxdApiClient::new(channel.clone()), request),
            )
            .await
            .map_err(|_| CloudError::Timeout { op, after: timeout })?;
            match outcome {
                Ok(response) => return Ok(response.into_inner()),
                Err(status) if status.code() == tonic::Code::Unauthenticated && attempt == 0 => {
                    self.tokens.invalidate().await;
                }
                Err(status) if status.code() == tonic::Code::DeadlineExceeded => {
                    return Err(CloudError::Timeout { op, after: timeout });
                }
                Err(status) => return Err(CloudError::rpc(op, &status)),
            }
        }
        Err(CloudError::Auth(format!("boxd refused the token for {op}")))
    }

    async fn create_once(
        &self,
        spec: &NewMachine,
        origin: &MachineOrigin,
        size: MachineSize,
    ) -> CloudResult<MachineInfo> {
        let machine = match self.create_call(spec, origin, size).await? {
            Some(machine) => machine_info(machine),
            // Older answers carry no machine; read it back.
            None => self
                .get(&spec.name)
                .await?
                .ok_or_else(|| CloudError::NotFound {
                    what: "machine",
                    name: spec.name.clone(),
                })?,
        };
        self.remember(&machine);
        Ok(machine)
    }

    /// The call that makes `spec`'s machine from `origin`: a fork on its
    /// parent's endpoint, anything else where machines are created.
    async fn create_call(
        &self,
        spec: &NewMachine,
        origin: &MachineOrigin,
        size: MachineSize,
    ) -> CloudResult<Option<proto::GetVmResponse>> {
        let endpoint = match origin {
            MachineOrigin::Fork(parent) => self.endpoint_of(parent),
            _ => self.create_endpoint(),
        };
        let machine = match origin {
            MachineOrigin::Fresh => {
                let request = create_vm_request(spec, size, &self.org);
                self.unary(
                    "CreateVm",
                    &endpoint,
                    request,
                    CREATE_TIMEOUT,
                    |mut c, r| async move { c.create_vm(r).await },
                )
                .await?
                .machine
            }
            MachineOrigin::Snapshot(snapshot) => {
                let request = restore_request(snapshot, spec, &self.org);
                self.unary(
                    "CreateVmFromSnapshot",
                    &endpoint,
                    request,
                    CREATE_TIMEOUT,
                    |mut c, r| async move { c.create_vm_from_snapshot(r).await },
                )
                .await?
                .machine
            }
            MachineOrigin::Fork(parent) => {
                let request = fork_request(parent, spec);
                self.unary(
                    "ForkVm",
                    &endpoint,
                    request,
                    CREATE_TIMEOUT,
                    |mut c, r| async move { c.fork_vm(r).await },
                )
                .await?
                .machine
            }
        };
        Ok(machine)
    }
}

#[cfg(test)]
mod tests;
