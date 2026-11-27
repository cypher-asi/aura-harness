use super::*;
use crate::hub::request::{chat_request, TurnAsk};
use crate::hub::turn::ws_url;

#[test]
fn choose_binary_prefers_env_then_cloud_home_then_running_exe() {
    let home = tempfile::tempdir().expect("tempdir");
    let exe = PathBuf::from("/usr/bin/aura-running");
    let env = PathBuf::from("/opt/aura-linux");

    // Nothing built in the cloud home: the running executable.
    assert_eq!(
        choose_binary(None, home.path(), Some(exe.clone())),
        AuraBinary::Upload(exe.clone())
    );
    assert_eq!(
        choose_binary(None, home.path(), None),
        AuraBinary::Installed
    );

    // A build in the cloud home wins over the running executable.
    let built = crate::snapshots::cloud_binary_path(home.path());
    std::fs::create_dir_all(built.parent().expect("parent")).expect("mkdir");
    std::fs::write(&built, b"elf").expect("write");
    assert_eq!(
        choose_binary(None, home.path(), Some(exe.clone())),
        AuraBinary::Upload(built)
    );

    // AURA_CLOUD_BINARY wins over both.
    assert_eq!(
        choose_binary(Some(&env), home.path(), Some(exe)),
        AuraBinary::Upload(env)
    );
}

#[test]
fn stream_frames_map_to_events() {
    assert_eq!(
        map_outbound(OutboundMessage::TextDelta(TextDelta { text: "hi".into() })),
        Some(RemoteEvent::Text("hi".into()))
    );
    assert_eq!(
        map_outbound(OutboundMessage::ToolUseStart(ToolUseStart {
            id: "1".into(),
            name: "read_file".into()
        })),
        Some(RemoteEvent::ToolStart {
            id: "1".into(),
            name: "read_file".into()
        })
    );
    let fatal = OutboundMessage::Error(ErrorMsg {
        code: "x".into(),
        message: "boom".into(),
        recoverable: false,
        support_id: None,
    });
    assert_eq!(
        map_outbound(fatal),
        Some(RemoteEvent::Failed("boom".into()))
    );
    let soft = OutboundMessage::Error(ErrorMsg {
        code: "x".into(),
        message: "retry".into(),
        recoverable: true,
        support_id: None,
    });
    assert_eq!(map_outbound(soft), None);
}

#[test]
fn urls_and_requests() {
    assert_eq!(
        ws_url("https://m.boxd.sh", "/v1/mesh"),
        "wss://m.boxd.sh/v1/mesh"
    );
    assert_eq!(
        ws_url("http://127.0.0.1:8080", "/stream/1"),
        "ws://127.0.0.1:8080/stream/1"
    );
    let ask = TurnAsk {
        model: Some("gate".into()),
        ..TurnAsk::default()
    };
    let request = chat_request(AgentId::new([1; 32]), &ask);
    assert_eq!(
        request.agent_identity.partition_id,
        Some(AgentId::new([1; 32]).to_hex())
    );
    assert_eq!(request.model.title_model, None);
    assert_eq!(
        request.workspace.project_path.as_deref(),
        Some(WORKSPACE_DIR)
    );
    assert_eq!(request.agent_identity.system_prompt, None, "the node's own");
    assert_eq!(request.tool_permissions, None);
    let json = serde_json::to_string(&request).expect("json");
    assert!(!json.contains("bxd_"), "{json}");
}

#[test]
fn a_turn_brings_its_agents_prompt_and_turns_off_what_it_leaves_out() {
    use aura_core_protocol::ToolStateWire;
    let ask = TurnAsk {
        system_prompt: Some("<channel slug=\"site\">…</channel>".into()),
        tools_off: vec!["write_file".into(), "run_command".into()],
        ..TurnAsk::default()
    };
    let request = chat_request(AgentId::new([1; 32]), &ask);
    let prompt = request
        .agent_identity
        .system_prompt
        .expect("its own prompt");
    assert!(
        prompt.starts_with("<channel slug=\"site\">…</channel>"),
        "{prompt}"
    );
    assert!(
        prompt.contains(&format!("Your workspace root is `{WORKSPACE_DIR}`")),
        "the node leaves that out of a prompt it is given: {prompt}"
    );
    let off = request.tool_permissions.expect("tools off").per_tool;
    assert_eq!(
        off.into_iter().collect::<Vec<_>>(),
        [
            ("run_command".to_string(), ToolStateWire::Off),
            ("write_file".to_string(), ToolStateWire::Off),
        ]
    );
}

#[test]
fn project_defaults_persist() {
    let (hub, _fake, dir) = hub();
    assert_eq!(hub.project_default("p1"), Placement::Local);
    hub.set_project_default("p1", Placement::Cloud);
    drop(hub);
    let store: Arc<dyn Store> =
        Arc::new(RocksStore::open(dir.path().join("db2"), false).expect("open"));
    let again = CloudHub::new(
        CloudConfig::defaults(),
        store,
        dir.path().join("cloud"),
        Arc::new(FakeProvider::new()),
        AuraBinary::Installed,
    )
    .expect("hub");
    assert_eq!(again.project_default("p1"), Placement::Cloud);
}
