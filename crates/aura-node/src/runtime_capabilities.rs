use aura_core::{
    InstalledIntegrationDefinition, InstalledToolCapability, InstalledToolDefinition,
    PermissionLevel, RuntimeCapabilityInstall, SystemKind, Transaction, TransactionType,
};
use aura_kernel::{Kernel, KernelError, PolicyConfig};
use aura_tools::domain_tools::DomainApi;
use std::collections::HashMap;
use std::sync::Arc;

/// Build a fresh [`PolicyConfig`] for a session.
///
/// * `installed_tools` / `installed_integrations` — declared capability
///   surface for this agent. Installed tools are added to
///   `allowed_tools` with their default permission level.
/// * `agent_permissions` — per-agent overrides from aura-network (see
///   [`fetch_agent_permissions_with_default`]). Every entry in this map
///   is spliced into [`PolicyConfig::tool_permissions`] **and** added to
///   [`PolicyConfig::allowed_tools`] so the fail-closed `allow_unlisted
///   = false` default from [`PolicyConfig::default`] does not silently
///   demote an explicit override back to `Deny`.
pub(crate) fn build_policy_config(
    installed_tools: &[InstalledToolDefinition],
    installed_integrations: &[InstalledIntegrationDefinition],
    agent_permissions: &HashMap<String, PermissionLevel>,
) -> PolicyConfig {
    let mut policy = PolicyConfig::default();
    policy.add_allowed_tools(installed_tools.iter().map(|tool| tool.name.clone()));
    policy.set_installed_integrations(installed_integrations.iter().cloned());
    policy.set_tool_integration_requirements(installed_tools.iter().filter_map(|tool| {
        tool.required_integration
            .clone()
            .map(|requirement| (tool.name.clone(), requirement))
    }));

    for (tool, level) in agent_permissions {
        if !matches!(level, PermissionLevel::Deny) {
            // Anything other than `Deny` must be in `allowed_tools` so
            // the `allow_unlisted = false` gate doesn't veto the
            // override. `Deny` is honored even for unlisted tools
            // (defense in depth).
            policy.allowed_tools.insert(tool.clone());
        }
        policy.tool_permissions.insert(tool.clone(), *level);
    }

    policy
}

/// Fetch per-agent tool permission overrides with the aura-os fallback
/// matrix applied.
///
/// The fallback matrix is:
///
/// | aura-network response | `strict_mode` off (default)                                        | `strict_mode` on |
/// |-----------------------|--------------------------------------------------------------------|------------------|
/// | `Ok(Some(map))`       | return `map` with permissive defaults merged in (non-destructive)  | filter out anything more permissive than the kernel default (see below) |
/// | `Ok(None)`            | return permissive defaults                                         | return empty map |
/// | `Err(_)`              | log + return empty map (fail closed)                               | log + return empty map (fail closed) |
///
/// Non-strict mode unconditionally surfaces `run_command: AlwaysAllow`
/// so aura-os deployments work out of the box without the operator
/// having to set env opt-ins (historically `AURA_AUTONOMOUS_DEV_LOOP=1`
/// / `AURA_ALLOW_RUN_COMMAND=1`). If aura-network's profile carries an
/// explicit entry for `run_command`, that entry wins — the merge is
/// additive and never overwrites what the domain API returned.
///
/// "More permissive than the default" in strict mode means: any entry
/// whose kernel default is stricter than what aura-network is asking
/// for is dropped. In practice the only default we actively downgrade
/// via this map is `run_command` (default `RequireApproval`) being
/// flipped to `AlwaysAllow`; strict mode keeps those elevations from
/// sneaking in while still allowing `Deny` or `RequireApproval` to come
/// through (those are strictly tighter than the default, never looser).
pub(crate) async fn fetch_agent_permissions_with_default(
    domain_api: Option<&Arc<dyn DomainApi>>,
    agent_id: Option<&str>,
    jwt: Option<&str>,
    strict_mode: bool,
) -> HashMap<String, PermissionLevel> {
    let (Some(api), Some(agent_id)) = (domain_api, agent_id) else {
        return if strict_mode {
            HashMap::new()
        } else {
            default_permissive_overrides()
        };
    };

    match api.get_agent_permissions(agent_id, jwt).await {
        Ok(Some(mut map)) => {
            if strict_mode {
                map.retain(|tool, level| is_tighter_or_equal_to_default(tool, *level));
            } else {
                merge_permissive_defaults(&mut map);
            }
            map
        }
        Ok(None) => {
            if strict_mode {
                HashMap::new()
            } else {
                default_permissive_overrides()
            }
        }
        Err(err) => {
            tracing::warn!(
                agent_id = %agent_id,
                error = %err,
                "fetch_agent_permissions failed; falling back to fail-closed empty overrides"
            );
            HashMap::new()
        }
    }
}

/// Permissive defaults applied in non-strict mode.
///
/// Keep this minimal — the only elevation here is `run_command` →
/// `AlwaysAllow` so aura-os spawned agents can execute shell commands
/// without per-call approval. Everything else continues to use the
/// kernel's own [`aura_kernel::default_tool_permission`] map.
fn default_permissive_overrides() -> HashMap<String, PermissionLevel> {
    let mut map = HashMap::new();
    map.insert("run_command".to_string(), PermissionLevel::AlwaysAllow);
    map
}

/// Merge [`default_permissive_overrides`] into `map`, preserving any
/// explicit entries the domain API already returned. Only keys missing
/// from `map` are inserted — an aura-network profile that deliberately
/// pins `run_command` to something stricter is honored verbatim.
fn merge_permissive_defaults(map: &mut HashMap<String, PermissionLevel>) {
    for (tool, level) in default_permissive_overrides() {
        map.entry(tool).or_insert(level);
    }
}

/// True when `level` is no looser than the kernel's built-in default
/// for `tool`. Used in `strict_mode` to reject aura-network trying to
/// elevate a tool beyond the kernel's baseline.
fn is_tighter_or_equal_to_default(tool: &str, level: PermissionLevel) -> bool {
    use aura_kernel::default_tool_permission;
    let default = default_tool_permission(tool);
    permissiveness(level) <= permissiveness(default)
}

/// Total ordering over [`PermissionLevel`] from least to most
/// permissive. `Deny` (0) < `RequireApproval` (1) < `AskOnce` (2) <
/// `AlwaysAllow` (3).
fn permissiveness(level: PermissionLevel) -> u8 {
    match level {
        PermissionLevel::Deny => 0,
        PermissionLevel::RequireApproval => 1,
        PermissionLevel::AskOnce => 2,
        PermissionLevel::AlwaysAllow => 3,
    }
}

pub(crate) async fn record_runtime_capabilities(
    kernel: &Kernel,
    scope: &str,
    session_id: Option<&str>,
    installed_tools: &[InstalledToolDefinition],
    installed_integrations: &[InstalledIntegrationDefinition],
) -> Result<(), KernelError> {
    kernel
        .process_direct(Transaction::session_start(kernel.agent_id))
        .await?;

    let payload = RuntimeCapabilityInstall {
        system_kind: SystemKind::CapabilityInstall,
        scope: scope.to_string(),
        session_id: session_id.map(str::to_string),
        installed_integrations: installed_integrations.to_vec(),
        installed_tools: installed_tools
            .iter()
            .map(InstalledToolCapability::from)
            .collect(),
    };
    let payload_bytes = serde_json::to_vec(&payload)
        .map_err(|e| KernelError::Serialization(format!("serialize capability install: {e}")))?;
    let tx = Transaction::new_chained(
        kernel.agent_id,
        TransactionType::System,
        payload_bytes,
        None,
    );
    kernel.process_direct(tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use aura_tools::domain_tools::{
        CreateSessionParams, DomainApi, MessageDescriptor, ProjectDescriptor, ProjectUpdate,
        SaveMessageParams, SessionDescriptor, SpecDescriptor, TaskDescriptor, TaskUpdate,
    };
    use std::sync::Mutex;

    /// Mock `DomainApi` that only implements `get_agent_permissions`;
    /// every other method returns `unimplemented!()`. The fixture
    /// records call counts so cache tests can verify hits.
    type MockResult = anyhow::Result<Option<HashMap<String, PermissionLevel>>>;
    type MockFn = Box<dyn FnMut() -> MockResult + Send>;

    struct MockDomain {
        result: Mutex<MockFn>,
    }

    impl MockDomain {
        fn new(f: impl FnMut() -> MockResult + Send + 'static) -> Self {
            Self {
                result: Mutex::new(Box::new(f)),
            }
        }
    }

    #[async_trait]
    impl DomainApi for MockDomain {
        async fn get_agent_permissions(
            &self,
            _agent_id: &str,
            _jwt: Option<&str>,
        ) -> anyhow::Result<Option<HashMap<String, PermissionLevel>>> {
            (self.result.lock().unwrap())()
        }

        // --- everything else: unreachable for these tests ---

        async fn list_specs(
            &self,
            _project_id: &str,
            _jwt: Option<&str>,
        ) -> anyhow::Result<Vec<SpecDescriptor>> {
            unimplemented!()
        }
        async fn get_spec(
            &self,
            _spec_id: &str,
            _jwt: Option<&str>,
        ) -> anyhow::Result<SpecDescriptor> {
            unimplemented!()
        }
        async fn create_spec(
            &self,
            _p: &str,
            _t: &str,
            _c: &str,
            _o: u32,
            _j: Option<&str>,
        ) -> anyhow::Result<SpecDescriptor> {
            unimplemented!()
        }
        async fn update_spec(
            &self,
            _id: &str,
            _t: Option<&str>,
            _c: Option<&str>,
            _j: Option<&str>,
        ) -> anyhow::Result<SpecDescriptor> {
            unimplemented!()
        }
        async fn delete_spec(&self, _id: &str, _j: Option<&str>) -> anyhow::Result<()> {
            unimplemented!()
        }
        async fn list_tasks(
            &self,
            _p: &str,
            _s: Option<&str>,
            _j: Option<&str>,
        ) -> anyhow::Result<Vec<TaskDescriptor>> {
            unimplemented!()
        }
        async fn create_task(
            &self,
            _p: &str,
            _s: &str,
            _t: &str,
            _d: &str,
            _deps: &[String],
            _o: u32,
            _j: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            unimplemented!()
        }
        async fn update_task(
            &self,
            _id: &str,
            _u: TaskUpdate,
            _j: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            unimplemented!()
        }
        async fn delete_task(&self, _id: &str, _j: Option<&str>) -> anyhow::Result<()> {
            unimplemented!()
        }
        async fn transition_task(
            &self,
            _id: &str,
            _s: &str,
            _j: Option<&str>,
        ) -> anyhow::Result<TaskDescriptor> {
            unimplemented!()
        }
        async fn claim_next_task(
            &self,
            _p: &str,
            _a: &str,
            _j: Option<&str>,
        ) -> anyhow::Result<Option<TaskDescriptor>> {
            unimplemented!()
        }
        async fn get_task(&self, _id: &str, _j: Option<&str>) -> anyhow::Result<TaskDescriptor> {
            unimplemented!()
        }
        async fn get_project(
            &self,
            _p: &str,
            _j: Option<&str>,
        ) -> anyhow::Result<ProjectDescriptor> {
            unimplemented!()
        }
        async fn update_project(
            &self,
            _p: &str,
            _u: ProjectUpdate,
            _j: Option<&str>,
        ) -> anyhow::Result<ProjectDescriptor> {
            unimplemented!()
        }
        async fn create_log(
            &self,
            _p: &str,
            _m: &str,
            _l: &str,
            _a: Option<&str>,
            _md: Option<&serde_json::Value>,
            _j: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            unimplemented!()
        }
        async fn list_logs(
            &self,
            _p: &str,
            _l: Option<&str>,
            _n: Option<u64>,
            _j: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            unimplemented!()
        }
        async fn get_project_stats(
            &self,
            _p: &str,
            _j: Option<&str>,
        ) -> anyhow::Result<serde_json::Value> {
            unimplemented!()
        }
        async fn list_messages(
            &self,
            _p: &str,
            _i: &str,
        ) -> anyhow::Result<Vec<MessageDescriptor>> {
            unimplemented!()
        }
        async fn save_message(&self, _p: SaveMessageParams) -> anyhow::Result<()> {
            unimplemented!()
        }
        async fn create_session(
            &self,
            _p: CreateSessionParams,
        ) -> anyhow::Result<SessionDescriptor> {
            unimplemented!()
        }
        async fn get_active_session(&self, _i: &str) -> anyhow::Result<Option<SessionDescriptor>> {
            unimplemented!()
        }
        async fn orbit_api_call(
            &self,
            _m: &str,
            _p: &str,
            _b: Option<&serde_json::Value>,
            _j: Option<&str>,
        ) -> anyhow::Result<String> {
            unimplemented!()
        }
        async fn network_api_call(
            &self,
            _m: &str,
            _p: &str,
            _b: Option<&serde_json::Value>,
            _j: Option<&str>,
        ) -> anyhow::Result<String> {
            unimplemented!()
        }
    }

    #[test]
    fn build_policy_config_adds_overridden_tool_to_allowed_set() {
        // An aura-os agent with `run_command` elevated to AlwaysAllow:
        // the policy must have both the tool_permissions override AND
        // the tool in allowed_tools so the fail-closed
        // `allow_unlisted = false` default doesn't veto it.
        let mut overrides = HashMap::new();
        overrides.insert("run_command".to_string(), PermissionLevel::AlwaysAllow);

        let policy = build_policy_config(&[], &[], &overrides);

        assert!(policy.allowed_tools.contains("run_command"));
        assert_eq!(
            policy.tool_permissions.get("run_command"),
            Some(&PermissionLevel::AlwaysAllow)
        );
    }

    #[test]
    fn build_policy_config_deny_override_not_added_to_allowed_tools() {
        // A `Deny` override is honored even for unlisted tools; we do
        // not need to put it in allowed_tools (and doing so would
        // imply the tool is generally permitted, which is misleading).
        let mut overrides = HashMap::new();
        overrides.insert("run_command".to_string(), PermissionLevel::Deny);

        let policy = build_policy_config(&[], &[], &overrides);

        assert!(!policy.allowed_tools.contains("run_command"));
        assert_eq!(
            policy.tool_permissions.get("run_command"),
            Some(&PermissionLevel::Deny)
        );
    }

    #[test]
    fn permissiveness_ordering_matches_trust_hierarchy() {
        assert!(
            permissiveness(PermissionLevel::Deny)
                < permissiveness(PermissionLevel::RequireApproval)
        );
        assert!(
            permissiveness(PermissionLevel::RequireApproval)
                < permissiveness(PermissionLevel::AskOnce)
        );
        assert!(
            permissiveness(PermissionLevel::AskOnce) < permissiveness(PermissionLevel::AlwaysAllow)
        );
    }

    #[test]
    fn is_tighter_or_equal_to_default_blocks_run_command_elevation() {
        // run_command defaults to RequireApproval; AlwaysAllow is looser.
        assert!(!is_tighter_or_equal_to_default(
            "run_command",
            PermissionLevel::AlwaysAllow
        ));
        assert!(is_tighter_or_equal_to_default(
            "run_command",
            PermissionLevel::RequireApproval
        ));
        assert!(is_tighter_or_equal_to_default(
            "run_command",
            PermissionLevel::Deny
        ));
    }

    #[tokio::test]
    async fn fetch_no_domain_api_non_strict_returns_permissive_defaults() {
        let map = fetch_agent_permissions_with_default(None, None, None, false).await;
        assert_eq!(map.get("run_command"), Some(&PermissionLevel::AlwaysAllow));
    }

    #[tokio::test]
    async fn fetch_no_domain_api_strict_returns_empty() {
        let map = fetch_agent_permissions_with_default(None, None, None, true).await;
        assert!(map.is_empty());
    }

    #[tokio::test]
    async fn fetch_404_non_strict_returns_permissive_defaults() {
        let mock: Arc<dyn DomainApi> = Arc::new(MockDomain::new(|| Ok(None)));
        let map =
            fetch_agent_permissions_with_default(Some(&mock), Some("agent-1"), None, false).await;
        assert_eq!(map.get("run_command"), Some(&PermissionLevel::AlwaysAllow));
    }

    #[tokio::test]
    async fn fetch_404_strict_returns_empty() {
        let mock: Arc<dyn DomainApi> = Arc::new(MockDomain::new(|| Ok(None)));
        let map =
            fetch_agent_permissions_with_default(Some(&mock), Some("agent-1"), None, true).await;
        assert!(map.is_empty());
    }

    #[tokio::test]
    async fn fetch_err_always_returns_empty_even_non_strict() {
        let mock: Arc<dyn DomainApi> = Arc::new(MockDomain::new(|| Err(anyhow::anyhow!("boom"))));
        let map =
            fetch_agent_permissions_with_default(Some(&mock), Some("agent-1"), None, false).await;
        assert!(
            map.is_empty(),
            "transport errors must fail closed regardless of strict_mode"
        );
    }

    #[tokio::test]
    async fn fetch_some_map_non_strict_passes_through() {
        let mut response = HashMap::new();
        response.insert("run_command".to_string(), PermissionLevel::AlwaysAllow);
        response.insert("write_file".to_string(), PermissionLevel::Deny);
        let response_clone = response.clone();
        let mock: Arc<dyn DomainApi> =
            Arc::new(MockDomain::new(move || Ok(Some(response_clone.clone()))));

        let map =
            fetch_agent_permissions_with_default(Some(&mock), Some("agent-1"), None, false).await;
        assert_eq!(map, response);
    }

    #[tokio::test]
    async fn fetch_some_map_non_strict_injects_missing_run_command() {
        // aura-network returns a profile that says nothing about
        // `run_command`. Non-strict mode must fill in the permissive
        // default so the kernel doesn't silently deny every shell
        // invocation — this is the root-cause fix for the
        // "Tool 'run_command' is not allowed" regression that used to
        // require `AURA_AUTONOMOUS_DEV_LOOP=1` on the harness.
        let mut response = HashMap::new();
        response.insert("write_file".to_string(), PermissionLevel::Deny);
        let response_clone = response.clone();
        let mock: Arc<dyn DomainApi> =
            Arc::new(MockDomain::new(move || Ok(Some(response_clone.clone()))));

        let map =
            fetch_agent_permissions_with_default(Some(&mock), Some("agent-1"), None, false).await;

        assert_eq!(map.get("run_command"), Some(&PermissionLevel::AlwaysAllow));
        assert_eq!(map.get("write_file"), Some(&PermissionLevel::Deny));
    }

    #[tokio::test]
    async fn fetch_some_map_non_strict_preserves_explicit_run_command_override() {
        // If aura-network deliberately pins `run_command` to something
        // stricter than AlwaysAllow, the merge must not overwrite it.
        let mut response = HashMap::new();
        response.insert(
            "run_command".to_string(),
            PermissionLevel::RequireApproval,
        );
        let response_clone = response.clone();
        let mock: Arc<dyn DomainApi> =
            Arc::new(MockDomain::new(move || Ok(Some(response_clone.clone()))));

        let map =
            fetch_agent_permissions_with_default(Some(&mock), Some("agent-1"), None, false).await;
        assert_eq!(
            map.get("run_command"),
            Some(&PermissionLevel::RequireApproval)
        );
    }

    #[tokio::test]
    async fn fetch_some_map_strict_drops_elevations() {
        // aura-network tries to elevate `run_command` to AlwaysAllow
        // and to tighten `write_file` to Deny. Strict mode must drop
        // the elevation but keep the tightening.
        let mut response = HashMap::new();
        response.insert("run_command".to_string(), PermissionLevel::AlwaysAllow);
        response.insert("write_file".to_string(), PermissionLevel::Deny);
        let response_clone = response.clone();
        let mock: Arc<dyn DomainApi> =
            Arc::new(MockDomain::new(move || Ok(Some(response_clone.clone()))));

        let map =
            fetch_agent_permissions_with_default(Some(&mock), Some("agent-1"), None, true).await;
        assert!(!map.contains_key("run_command"));
        assert_eq!(map.get("write_file"), Some(&PermissionLevel::Deny));
    }
}
