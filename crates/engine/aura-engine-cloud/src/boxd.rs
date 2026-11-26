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

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aura_config::{CloudConfig, MachineSize};
use aura_core_types::{VmSource, VmUsage};
use futures_util::{stream, StreamExt as _};
use parking_lot::Mutex;
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

use crate::error::{CloudError, CloudResult};
use crate::proto::{self, boxd_api_client::BoxdApiClient};
use crate::provider::{
    Balance, ExecOutput, HostSecret, MachineInfo, MachineLimit, MachineOrigin, MachineStatus,
    NewMachine, OrgUsage, SnapshotInfo, UsageReading, VmProvider, MACHINE_DISK_BYTES,
};
use crate::token::{exchange_url_for, TokenSource};

/// Limit on ordinary calls.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Limit on creating, forking and snapshotting.
pub const CREATE_TIMEOUT: Duration = Duration::from_secs(180);
/// Limit on connecting to an endpoint.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest chunk sent in one upload message (boxd caps a message at
/// 4 MiB).
const UPLOAD_CHUNK: usize = 1024 * 1024;
/// How often an upload says how far it has got.
const UPLOAD_REPORT_EVERY: Duration = Duration::from_millis(500);

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
        let endpoint = match origin {
            MachineOrigin::Fork(parent) => self.endpoint_of(parent),
            _ => self.create_endpoint(),
        };
        let labels: HashMap<String, String> = spec.labels.clone().into_iter().collect();
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
                let request = proto::ForkVmRequest {
                    source_vm_id: parent.clone(),
                    name: spec.name.clone(),
                    isolated: spec.isolated,
                    labels,
                    networks: spec.networks.clone(),
                    delete_after_secs: spec.delete_after_secs,
                    ..Default::default()
                };
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
        let machine = match machine {
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
}

fn normalize_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.contains("://") {
        trimmed.to_string()
    } else if trimmed.starts_with("localhost") || trimmed.starts_with("127.") {
        format!("http://{trimmed}")
    } else {
        format!("https://{trimmed}")
    }
}

/// The `CreateVm` request for a fresh machine.
#[must_use]
pub fn create_vm_request(
    spec: &NewMachine,
    size: MachineSize,
    org: &str,
) -> proto::CreateVmRequest {
    let machine_spec = proto::MachineSpec {
        version: 1,
        size: size.as_spec().to_string(),
        labels: spec.labels.clone().into_iter().collect(),
        networks: if spec.isolated {
            Vec::new()
        } else {
            spec.networks.clone()
        },
        isolated: spec.isolated,
        env: spec.env.clone().into_iter().collect(),
        packages: spec.packages.clone(),
        secrets: spec.secrets.clone(),
        ..Default::default()
    };
    proto::CreateVmRequest {
        name: spec.name.clone(),
        org: org.to_string(),
        spec: Some(machine_spec),
        delete_after_secs: spec.delete_after_secs,
        ..Default::default()
    }
}

/// The `CreateVmFromSnapshot` request. Secrets go in a spec merged over
/// the snapshot's, which then also carries the labels and isolation
/// (boxd refuses them beside a spec).
#[must_use]
pub fn restore_request(
    snapshot: &str,
    spec: &NewMachine,
    org: &str,
) -> proto::CreateVmFromSnapshotRequest {
    let mut request = proto::CreateVmFromSnapshotRequest {
        snapshot: snapshot.to_string(),
        name: spec.name.clone(),
        org: org.to_string(),
        delete_after_secs: spec.delete_after_secs,
        ..Default::default()
    };
    if spec.secrets.is_empty() {
        request.isolated = spec.isolated;
        request.labels = spec.labels.clone().into_iter().collect();
    } else {
        request.spec = Some(proto::MachineSpec {
            version: 1,
            labels: spec.labels.clone().into_iter().collect(),
            isolated: spec.isolated,
            secrets: spec.secrets.clone(),
            ..Default::default()
        });
    }
    request
}

/// The org a key keeps secrets in: its default org, or its only one, by
/// slug (`SetVar` refuses org ids: "org not found").
#[must_use]
pub fn pick_org(answer: &proto::ListOrgsResponse) -> Option<String> {
    let slug = |org: &proto::OrgSummary| {
        if org.slug.is_empty() {
            org.name.clone()
        } else {
            org.slug.clone()
        }
    };
    if let Some(org) = answer
        .orgs
        .iter()
        .find(|org| org.id == answer.default_org_id)
    {
        return Some(slug(org));
    }
    match answer.orgs.as_slice() {
        [only] => Some(slug(only)),
        _ => None,
    }
}

/// The `SetVar` request for an unattached, host-bound secret.
#[must_use]
pub fn set_secret_request(secret: &HostSecret, org: &str) -> proto::SetVarRequest {
    proto::SetVarRequest {
        org: org.to_string(),
        kind: "secret".into(),
        name: secret.name.clone(),
        value: secret.value.as_str().to_string(),
        scope: "private-unattached".into(),
        domains: secret.domains.clone(),
        replace_in: secret.replace_in.clone(),
    }
}

/// Map boxd's machine answer.
#[must_use]
pub fn machine_info(vm: proto::GetVmResponse) -> MachineInfo {
    // `access_domain` is the cluster's domain (`boxd.sh`); the machine
    // is served at `<name>.<access_domain>`.
    let base = if vm.access_domain.is_empty() {
        "boxd.sh"
    } else {
        vm.access_domain.as_str()
    };
    let domain = if base.starts_with(&format!("{}.", vm.name)) {
        base.to_string()
    } else {
        format!("{}.{base}", vm.name)
    };
    let source = vm.source_info.as_ref().and_then(|source| {
        match source.kind.to_ascii_lowercase().as_str() {
            "snapshot" => Some(VmSource::Snapshot {
                name: source.name.clone(),
                version: u32::try_from(source.version).ok().filter(|v| *v > 0),
            }),
            "fork" => Some(VmSource::Fork {
                parent: source.name.clone(),
            }),
            "image" | "fresh" | "" => Some(VmSource::Image {
                image: vm.image_ref.clone(),
            }),
            _ => None,
        }
    });
    MachineInfo {
        id: vm.vm_id,
        name: vm.name,
        status: MachineStatus::parse(&vm.status),
        region: vm.region,
        vcpu: vm.vcpu,
        memory_bytes: vm.memory_bytes,
        disk_bytes: if vm.disk_bytes == 0 {
            MACHINE_DISK_BYTES
        } else {
            vm.disk_bytes
        },
        domain,
        public_ip: vm.public_ip,
        ssh_port: vm.ssh_port,
        isolated: vm.isolated,
        labels: vm.labels.into_iter().collect::<BTreeMap<_, _>>(),
        source,
        created_ms: millis_from(vm.created_at),
        boot_ms: u64::try_from(vm.boot_time_ms).unwrap_or(0),
    }
}

/// boxd timestamps are Unix seconds or milliseconds depending on the
/// field; anything below year 2100 in seconds is taken as seconds.
fn millis_from(raw: i64) -> u64 {
    let raw = u64::try_from(raw).unwrap_or(0);
    if raw < 4_102_444_800 {
        raw * 1000
    } else {
        raw
    }
}

/// Map boxd's usage answer.
#[must_use]
pub fn vm_usage(usage: &proto::VmUsage, until_ms: u64) -> VmUsage {
    VmUsage {
        vcpu_hours: usage.vcpu_hours,
        ram_gib_hours: usage.ram_gib_hours,
        disk_gib_hours: usage.disk_gib_hours,
        cost_micro: usage.cost_micro,
        complete: usage.complete,
        until_ms,
    }
}

fn snapshot_info(info: proto::SnapshotInfo) -> SnapshotInfo {
    SnapshotInfo {
        id: info.snapshot_id,
        name: info.name,
        version: info.version,
        status: info.status,
        size_bytes: info.size_bytes,
        vcpu: info.vcpu,
    }
}

#[async_trait]
impl VmProvider for BoxdProvider {
    async fn create(&self, spec: &NewMachine, origin: &MachineOrigin) -> CloudResult<MachineInfo> {
        let mut size = spec.size;
        loop {
            match self.create_once(spec, origin, size).await {
                Err(err) if *origin == MachineOrigin::Fresh && err.is_size_refused() => {
                    let Some(smaller) = size.smaller() else {
                        return Err(err);
                    };
                    tracing::warn!(
                        machine = %spec.name,
                        refused = %size,
                        fallback = %smaller,
                        "boxd refused the machine size; trying one size down"
                    );
                    size = smaller;
                }
                other => return other,
            }
        }
    }

    async fn get(&self, machine: &str) -> CloudResult<Option<MachineInfo>> {
        let request = proto::GetVmRequest {
            vm_id: machine.to_string(),
        };
        match self
            .unary(
                "GetVm",
                &self.endpoint_of(machine),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.get_vm(r).await },
            )
            .await
        {
            Ok(vm) => {
                let info = machine_info(vm);
                self.remember(&info);
                Ok(Some(info))
            }
            Err(err) if err.is_not_found() => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn list(&self, label_selector: &[String]) -> CloudResult<Vec<MachineInfo>> {
        let request = proto::ListVmsRequest {
            org: self.org.clone(),
            label_selector: label_selector.to_vec(),
            ..Default::default()
        };
        let answer = self
            .unary(
                "ListVms",
                &self.create_endpoint(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.list_vms(r).await },
            )
            .await?;
        let machines: Vec<MachineInfo> = answer.vms.into_iter().map(machine_info).collect();
        for machine in &machines {
            self.remember(machine);
        }
        Ok(machines)
    }

    async fn destroy(&self, machine: &str) -> CloudResult<()> {
        let request = proto::DestroyVmRequest {
            vm_id: machine.to_string(),
        };
        match self
            .unary(
                "DestroyVm",
                &self.endpoint_of(machine),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.destroy_vm(r).await },
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(err) if err.is_not_found() => Ok(()),
            Err(err) => Err(err),
        }
    }

    async fn hibernate(&self, machine: &str) -> CloudResult<()> {
        let request = proto::HibernateVmRequest {
            vm_id: machine.to_string(),
        };
        self.unary(
            "HibernateVm",
            &self.endpoint_of(machine),
            request,
            CALL_TIMEOUT,
            |mut c, r| async move { c.hibernate_vm(r).await },
        )
        .await
        .map(|_| ())
    }

    async fn wake(&self, machine: &str) -> CloudResult<()> {
        let request = proto::WakeVmRequest {
            vm_id: machine.to_string(),
        };
        self.unary(
            "WakeVm",
            &self.endpoint_of(machine),
            request,
            CALL_TIMEOUT,
            |mut c, r| async move { c.wake_vm(r).await },
        )
        .await
        .map(|_| ())
    }

    async fn standby(&self, machine: &str) -> CloudResult<()> {
        let request = proto::SuspendVmRequest {
            vm_id: machine.to_string(),
        };
        self.unary(
            "SuspendVm",
            &self.endpoint_of(machine),
            request,
            CALL_TIMEOUT,
            |mut c, r| async move { c.suspend_vm(r).await },
        )
        .await
        .map(|_| ())
    }

    async fn set_auto_hibernate(&self, machine: &str, secs: u32) -> CloudResult<()> {
        let request = proto::SetAutoHibernateTimeoutRequest {
            vm_id: machine.to_string(),
            timeout_secs: secs,
        };
        self.unary(
            "SetAutoHibernateTimeout",
            &self.endpoint_of(machine),
            request,
            CALL_TIMEOUT,
            |mut c, r| async move { c.set_auto_hibernate_timeout(r).await },
        )
        .await
        .map(|_| ())
    }

    async fn set_bot_protection(&self, machine: &str, enabled: bool) -> CloudResult<()> {
        let request = proto::SetBotProtectionRequest {
            vm_id: machine.to_string(),
            enabled,
            mode: if enabled { "on" } else { "off" }.to_string(),
        };
        self.unary(
            "SetBotProtection",
            &self.endpoint_of(machine),
            request,
            CALL_TIMEOUT,
            |mut c, r| async move { c.set_bot_protection(r).await },
        )
        .await
        .map(|_| ())
    }

    async fn exec(
        &self,
        machine: &str,
        command: &str,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
    ) -> CloudResult<ExecOutput> {
        self.exec_streaming(machine, command, stdin, timeout, &mut |_, _| {})
            .await
    }

    async fn exec_streaming(
        &self,
        machine: &str,
        command: &str,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        on_chunk: &mut (dyn for<'chunk> FnMut(bool, &'chunk [u8]) + Send),
    ) -> CloudResult<ExecOutput> {
        const OP: &str = "Exec";
        let channel = self.channel(&self.endpoint_of(machine))?;
        let mut chunks = vec![proto::ExecChunk {
            vm_id: machine.to_string(),
            command: command.to_string(),
            stdin: stdin.is_some(),
            ..Default::default()
        }];
        if let Some(input) = stdin.filter(|input| !input.is_empty()) {
            chunks.push(proto::ExecChunk {
                data: input,
                stdin: true,
                ..Default::default()
            });
        }
        let request = self.request(stream::iter(chunks), timeout).await?;
        let run = async {
            let mut answer = BoxdApiClient::new(channel)
                .exec(request)
                .await
                .map_err(|status| CloudError::rpc(OP, &status))?
                .into_inner();
            let mut output = ExecOutput::default();
            while let Some(chunk) = answer
                .message()
                .await
                .map_err(|status| CloudError::rpc(OP, &status))?
            {
                if !chunk.data.is_empty() {
                    on_chunk(chunk.is_stderr, &chunk.data);
                }
                if chunk.is_stderr {
                    output.stderr.extend_from_slice(&chunk.data);
                } else {
                    output.stdout.extend_from_slice(&chunk.data);
                }
                output.exit_code = chunk.exit_code;
            }
            Ok::<_, CloudError>(output)
        };
        tokio::time::timeout(timeout, run)
            .await
            .map_err(|_| CloudError::Timeout {
                op: OP,
                after: timeout,
            })?
    }

    async fn upload(&self, machine: &str, path: &str, bytes: Vec<u8>) -> CloudResult<u64> {
        self.upload_reported(machine, path, bytes, &mut |_, _| {})
            .await
    }

    async fn upload_reported(
        &self,
        machine: &str,
        path: &str,
        bytes: Vec<u8>,
        on_sent: &mut (dyn FnMut(u64, u64) + Send),
    ) -> CloudResult<u64> {
        const OP: &str = "UploadFileStream";
        let total = bytes.len() as u64;
        // Counted as tonic takes each chunk off the stream to send it:
        // with HTTP/2 flow control that tracks what went out closely.
        let sent = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut chunks = vec![proto::UploadFileChunk {
            vm_id: machine.to_string(),
            path: path.to_string(),
            total_size: total,
            data: Vec::new(),
        }];
        chunks.extend(
            bytes
                .chunks(UPLOAD_CHUNK)
                .map(|data| proto::UploadFileChunk {
                    data: data.to_vec(),
                    ..Default::default()
                }),
        );
        let counter = Arc::clone(&sent);
        let chunks = stream::iter(chunks).inspect(move |chunk| {
            counter.fetch_add(
                chunk.data.len() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
        });
        let channel = self.channel(&self.endpoint_of(machine))?;
        let request = self.request(chunks, CREATE_TIMEOUT).await?;
        let mut client = BoxdApiClient::new(channel);
        let call = tokio::time::timeout(CREATE_TIMEOUT, client.upload_file_stream(request));
        tokio::pin!(call);
        let mut every = tokio::time::interval(UPLOAD_REPORT_EVERY);
        let answer = loop {
            tokio::select! {
                answer = &mut call => break answer,
                _ = every.tick() => {
                    on_sent(sent.load(std::sync::atomic::Ordering::Relaxed).min(total), total);
                }
            }
        };
        let written = answer
            .map_err(|_| CloudError::Timeout {
                op: OP,
                after: CREATE_TIMEOUT,
            })?
            .map_err(|status| CloudError::rpc(OP, &status))?
            .into_inner()
            .bytes_written;
        if written != total {
            return Err(CloudError::InvalidResponse {
                op: OP,
                message: format!("wrote {written} of {total} bytes to {path}"),
            });
        }
        on_sent(total, total);
        Ok(written)
    }

    async fn download(&self, machine: &str, path: &str, max_bytes: u64) -> CloudResult<Vec<u8>> {
        const OP: &str = "DownloadFileStream";
        let channel = self.channel(&self.endpoint_of(machine))?;
        let request = self
            .request(
                proto::DownloadFileRequest {
                    vm_id: machine.to_string(),
                    path: path.to_string(),
                },
                CREATE_TIMEOUT,
            )
            .await?;
        let run = async {
            let mut answer = BoxdApiClient::new(channel)
                .download_file_stream(request)
                .await
                .map_err(|status| CloudError::rpc(OP, &status))?
                .into_inner();
            let mut total = None;
            let mut bytes = Vec::new();
            while let Some(chunk) = answer
                .message()
                .await
                .map_err(|status| CloudError::rpc(OP, &status))?
            {
                if total.is_none() {
                    if chunk.total_size > max_bytes {
                        return Err(CloudError::InvalidResponse {
                            op: OP,
                            message: format!(
                                "{path} is {} bytes, above the {max_bytes} byte limit",
                                chunk.total_size
                            ),
                        });
                    }
                    total = Some(chunk.total_size);
                }
                bytes.extend_from_slice(&chunk.data);
            }
            let expected = total.unwrap_or(0);
            if bytes.len() as u64 != expected {
                return Err(CloudError::InvalidResponse {
                    op: OP,
                    message: format!("got {} of {expected} bytes of {path}", bytes.len()),
                });
            }
            Ok(bytes)
        };
        tokio::time::timeout(CREATE_TIMEOUT, run)
            .await
            .map_err(|_| CloudError::Timeout {
                op: OP,
                after: CREATE_TIMEOUT,
            })?
    }

    async fn save_snapshot(&self, machine: &str, name: &str) -> CloudResult<SnapshotInfo> {
        let request = proto::CreateSnapshotRequest {
            vm: machine.to_string(),
            name: name.to_string(),
        };
        let answer = self
            .unary(
                "CreateSnapshot",
                &self.endpoint_of(machine),
                request,
                CREATE_TIMEOUT,
                |mut c, r| async move { c.create_snapshot(r).await },
            )
            .await?;
        Ok(SnapshotInfo {
            id: answer.snapshot_id,
            name: answer.name,
            version: answer.version,
            status: answer.status,
            size_bytes: 0,
            vcpu: 0,
        })
    }

    async fn snapshot(&self, name: &str) -> CloudResult<Option<SnapshotInfo>> {
        let request = proto::GetSnapshotRequest {
            name: name.to_string(),
            org: self.org.clone(),
        };
        match self
            .unary(
                "GetSnapshot",
                &self.create_endpoint(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.get_snapshot(r).await },
            )
            .await
        {
            Ok(answer) => Ok(answer.snapshot.map(snapshot_info)),
            Err(err) if err.is_not_found() => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn delete_snapshot(&self, name: &str) -> CloudResult<()> {
        let request = proto::DeleteSnapshotRequest {
            name: name.to_string(),
            org: self.org.clone(),
        };
        match self
            .unary(
                "DeleteSnapshot",
                &self.create_endpoint(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.delete_snapshot(r).await },
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(err) if err.is_not_found() => Ok(()),
            Err(err) => Err(err),
        }
    }

    async fn machine_limit(&self) -> CloudResult<MachineLimit> {
        let request = proto::GetOrgMachineLimitRequest {
            org_id: self.org.clone(),
        };
        let answer = self
            .unary(
                "GetOrgMachineLimit",
                &self.create_endpoint(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.get_org_machine_limit(r).await },
            )
            .await?;
        Ok(MachineLimit {
            count: answer.vm_count,
            max: answer.max_vms,
        })
    }

    async fn machine_usage(&self, machine: &str) -> CloudResult<UsageReading> {
        const OP: &str = "GetVmUsage";
        let request = proto::GetVmUsageRequest {
            vm_id: machine.to_string(),
            org: self.org.clone(),
            ..Default::default()
        };
        let answer = self
            .unary(
                OP,
                &self.home(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.get_vm_usage(r).await },
            )
            .await?;
        let until_ms = millis_from(answer.period_end);
        let usage = answer.usage.ok_or(CloudError::InvalidResponse {
            op: OP,
            message: "no usage in the answer".into(),
        })?;
        Ok(UsageReading {
            usage: vm_usage(&usage, until_ms),
            currency: answer.currency,
        })
    }

    async fn org_usage(&self, since_ms: u64) -> CloudResult<OrgUsage> {
        let request = proto::GetOrgUsageRequest {
            org: self.org.clone(),
            since: i64::try_from(since_ms / 1000).unwrap_or(0),
            until: 0,
        };
        let answer = self
            .unary(
                "GetOrgUsage",
                &self.home(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.get_org_usage(r).await },
            )
            .await?;
        let until_ms = millis_from(answer.period_end);
        Ok(OrgUsage {
            total_cost_micro: answer.total_cost_micro,
            currency: answer.currency,
            complete: answer.complete,
            machines: answer
                .vms
                .iter()
                .map(|vm| (vm.name.clone(), vm_usage(vm, until_ms)))
                .collect(),
        })
    }

    async fn set_secret(&self, secret: &HostSecret) -> CloudResult<()> {
        let org = self.secret_org().await?;
        let request = set_secret_request(secret, &org);
        self.unary(
            "SetVar",
            &self.home(),
            request,
            CALL_TIMEOUT,
            |mut c, r| async move { c.set_var(r).await },
        )
        .await?;
        Ok(())
    }

    async fn remove_secret(&self, name: &str) -> CloudResult<()> {
        let org = self.secret_org().await?;
        let request = proto::RemoveVarRequest {
            org,
            kind: "secret".into(),
            name: name.to_string(),
            scope: "private-unattached".into(),
        };
        match self
            .unary(
                "RemoveVar",
                &self.home(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.remove_var(r).await },
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(err) if err.is_not_found() => Ok(()),
            Err(err) => Err(err),
        }
    }

    async fn balance(&self) -> CloudResult<Balance> {
        let request = proto::ListBillingActivityRequest {
            org: self.org.clone(),
            page_size: 1,
            ..Default::default()
        };
        let answer = self
            .unary(
                "ListBillingActivity",
                &self.home(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.list_billing_activity(r).await },
            )
            .await?;
        Ok(Balance {
            currency: answer.currency,
            balance_micro: answer.balance_micro,
        })
    }
}

#[cfg(test)]
mod tests;
