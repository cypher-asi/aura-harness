//! boxd's messages: the requests this client builds and how its answers
//! map to the provider's domain types.

use std::collections::BTreeMap;

use aura_config::MachineSize;
use aura_core_types::{VmSource, VmUsage};

use crate::proto;
use crate::provider::{
    HostSecret, MachineInfo, MachineStatus, NewMachine, SnapshotInfo, MACHINE_DISK_BYTES,
};

pub(super) fn normalize_url(raw: &str) -> String {
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

/// The `ForkVm` request copying `parent` as `spec`'s machine.
pub(super) fn fork_request(parent: &str, spec: &NewMachine) -> proto::ForkVmRequest {
    proto::ForkVmRequest {
        source_vm_id: parent.to_string(),
        name: spec.name.clone(),
        isolated: spec.isolated,
        labels: spec.labels.clone().into_iter().collect(),
        networks: spec.networks.clone(),
        delete_after_secs: spec.delete_after_secs,
        ..Default::default()
    }
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
        last_connected_ms: u64::try_from(vm.last_connected_ms).unwrap_or(0),
    }
}

/// boxd timestamps are Unix seconds or milliseconds depending on the
/// field; anything below year 2100 in seconds is taken as seconds.
pub(super) fn millis_from(raw: i64) -> u64 {
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

pub(super) fn snapshot_info(info: proto::SnapshotInfo) -> SnapshotInfo {
    SnapshotInfo {
        id: info.snapshot_id,
        name: info.name,
        version: info.version,
        status: info.status,
        size_bytes: info.size_bytes,
        vcpu: info.vcpu,
    }
}
