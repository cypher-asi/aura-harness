use aura_core::{
    InstalledIntegrationDefinition, InstalledToolCapability, InstalledToolDefinition,
    RuntimeCapabilityInstall, SystemKind, Transaction, TransactionType,
};
use aura_kernel::{Kernel, KernelError, PolicyConfig};

pub(crate) fn build_policy_config(
    installed_tools: &[InstalledToolDefinition],
    installed_integrations: &[InstalledIntegrationDefinition],
) -> PolicyConfig {
    let mut policy = PolicyConfig::default();
    policy.add_allowed_tools(installed_tools.iter().map(|tool| tool.name.clone()));
    policy.set_installed_integrations(installed_integrations.iter().cloned());
    policy.set_tool_integration_requirements(installed_tools.iter().filter_map(|tool| {
        tool.required_integration
            .clone()
            .map(|requirement| (tool.name.clone(), requirement))
    }));
    policy
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
