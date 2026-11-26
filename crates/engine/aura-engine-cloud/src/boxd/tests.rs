use super::*;

#[test]
fn region_endpoints() {
    assert_eq!(
        region_endpoint("https://boxd.sh:9443", Some("nyc")),
        "https://nyc.boxd.sh:9443"
    );
    assert_eq!(
        region_endpoint("https://boxd.sh:9443", None),
        "https://boxd.sh:9443"
    );
    assert_eq!(
        region_endpoint("https://boxd.sh:9443", Some("")),
        "https://boxd.sh:9443"
    );
    assert_eq!(
        region_endpoint("https://boxd.example.com:9443", Some("nyc")),
        "https://boxd.example.com:9443"
    );
}

#[test]
fn urls_are_normalized() {
    assert_eq!(normalize_url("boxd.sh:9443"), "https://boxd.sh:9443");
    assert_eq!(normalize_url("localhost:9443/"), "http://localhost:9443");
    assert_eq!(
        normalize_url("https://boxd.sh:9443"),
        "https://boxd.sh:9443"
    );
}

#[test]
fn a_fresh_machine_request_carries_the_spec() {
    let mut spec = NewMachine::named("aura-base-1");
    spec.packages = vec!["apt-get update".into()];
    spec.labels.insert("aura.install".into(), "abc".into());
    spec.env.insert("AURA_NODE_PORT".into(), "8080".into());
    spec.networks = vec!["aura-p1".into()];
    spec.delete_after_secs = Some(7200);
    let request = create_vm_request(&spec, MachineSize::S4x16, "acme");
    let machine_spec = request.spec.expect("spec");
    assert_eq!(machine_spec.size, "4x16");
    assert!(machine_spec.isolated);
    assert!(
        machine_spec.networks.is_empty(),
        "isolated machines join no network"
    );
    assert_eq!(machine_spec.packages, vec!["apt-get update"]);
    assert_eq!(
        machine_spec.labels.get("aura.install").map(String::as_str),
        Some("abc")
    );
    assert_eq!(request.org, "acme");
    assert_eq!(request.delete_after_secs, Some(7200));
    // Fields the spec covers must not be set beside it.
    assert!(!request.isolated && request.labels.is_empty() && request.env.is_empty());
}

#[test]
fn machine_answers_map() {
    let vm = proto::GetVmResponse {
        vm_id: "id-1".into(),
        name: "aura-re36-3f9c".into(),
        status: "suspended".into(),
        vcpu: 4,
        memory_bytes: 16 << 30,
        region: "nyc".into(),
        isolated: true,
        created_at: 1_700_000_000,
        boot_time_ms: 6,
        source_info: Some(proto::VmSource {
            kind: "snapshot".into(),
            name: "aura-proj-x".into(),
            version: 4,
            id: String::new(),
        }),
        ..Default::default()
    };
    let info = machine_info(vm.clone());
    assert_eq!(info.status, MachineStatus::Standby);
    assert_eq!(info.domain, "aura-re36-3f9c.boxd.sh");
    let live = machine_info(proto::GetVmResponse {
        access_domain: "boxd.sh".into(),
        ..vm.clone()
    });
    assert_eq!(
        live.domain, "aura-re36-3f9c.boxd.sh",
        "boxd sends the cluster domain"
    );
    assert_eq!(live.url(), "https://aura-re36-3f9c.boxd.sh");
    let full = machine_info(proto::GetVmResponse {
        access_domain: "aura-re36-3f9c.example.dev".into(),
        ..vm
    });
    assert_eq!(full.domain, "aura-re36-3f9c.example.dev");
    assert_eq!(info.disk_bytes, MACHINE_DISK_BYTES);
    assert_eq!(info.created_ms, 1_700_000_000_000);
    assert_eq!(info.size(), Some(MachineSize::S4x16));
    assert_eq!(
        info.source,
        Some(VmSource::Snapshot {
            name: "aura-proj-x".into(),
            version: Some(4)
        })
    );
}

#[test]
fn usage_maps() {
    let usage = proto::VmUsage {
        vcpu_hours: 0.5,
        cost_micro: 410_000,
        complete: true,
        ..Default::default()
    };
    let mapped = vm_usage(&usage, 9);
    assert!(mapped.complete);
    assert_eq!(mapped.cost_micro, 410_000);
    assert_eq!(mapped.until_ms, 9);
}

#[test]
fn size_refusals_are_recognized() {
    let refused = CloudError::rpc(
        "CreateVm",
        &tonic::Status::invalid_argument("size 8x32 is above the org quota ceiling"),
    );
    assert!(refused.is_size_refused());
    let other = CloudError::rpc("CreateVm", &tonic::Status::invalid_argument("bad name"));
    assert!(!other.is_size_refused());
}

#[test]
fn secrets_map_to_unattached_host_bound_vars() {
    let secret = HostSecret {
        name: "AURA_3F9C0A1B_AB12CD_CLAUDE".into(),
        value: zeroize::Zeroizing::new("sk-ant-oat-x".into()),
        domains: vec!["api.anthropic.com".into()],
        replace_in: vec!["headers".into()],
    };
    let request = set_secret_request(&secret, "org-1");
    assert_eq!(request.kind, "secret");
    assert_eq!(request.scope, "private-unattached");
    assert_eq!(request.org, "org-1");
    assert_eq!(request.domains, ["api.anthropic.com"]);
    assert_eq!(request.replace_in, ["headers"]);
    assert!(
        !format!("{secret:?}").contains("sk-ant"),
        "Debug hides the value"
    );
}

#[test]
fn restores_with_secrets_put_labels_in_the_spec() {
    let mut spec = NewMachine::named("aura-re36-3f9c");
    spec.labels.insert("aura.owner".into(), "3f9c0a1b".into());
    let plain = restore_request("aura-proj-x", &spec, "");
    assert!(plain.spec.is_none());
    assert_eq!(
        plain.labels.get("aura.owner").map(String::as_str),
        Some("3f9c0a1b")
    );
    spec.secrets = vec!["AURA_X_Y_CLAUDE".into()];
    let with = restore_request("aura-proj-x", &spec, "");
    assert!(
        with.labels.is_empty() && !with.isolated,
        "boxd refuses them beside a spec"
    );
    let inner = with.spec.expect("spec");
    assert_eq!(inner.secrets, ["AURA_X_Y_CLAUDE"]);
    assert!(inner.isolated);
    assert_eq!(
        inner.labels.get("aura.owner").map(String::as_str),
        Some("3f9c0a1b")
    );
    let fresh = create_vm_request(&spec, MachineSize::S1x4, "");
    assert_eq!(fresh.spec.expect("spec").secrets, ["AURA_X_Y_CLAUDE"]);
}

#[test]
fn the_secret_org_is_the_default_or_the_only_one() {
    let org = |id: &str| proto::OrgSummary {
        id: id.into(),
        slug: format!("{id}-slug"),
        ..Default::default()
    };
    let answer = proto::ListOrgsResponse {
        orgs: vec![org("a"), org("b")],
        default_org_id: "b".into(),
    };
    assert_eq!(pick_org(&answer).as_deref(), Some("b-slug"));
    let one = proto::ListOrgsResponse {
        orgs: vec![org("a")],
        default_org_id: String::new(),
    };
    assert_eq!(pick_org(&one).as_deref(), Some("a-slug"));
    let many = proto::ListOrgsResponse {
        orgs: vec![org("a"), org("b")],
        default_org_id: String::new(),
    };
    assert_eq!(pick_org(&many), None);
}
