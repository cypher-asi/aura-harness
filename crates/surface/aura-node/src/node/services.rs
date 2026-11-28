//! Construction of the services the HTTP router serves: tools, model
//! providers, scheduler, automaton runtime, skills and sealed stores.

use super::{build_automaton, build_memory_manager, Node};
use crate::cloud_node::PlanResolver;
use crate::gateway::RouterState;
use anyhow::Context;
use aura_agent_kernel::Executor;
use aura_context_skills::{SkillInstallStore, SkillLoader, SkillManager};
use aura_engine::scheduler::Scheduler;
use aura_exec_domain_http::HttpDomainApi;
use aura_exec_tools::catalog::ToolProfile;
use aura_exec_tools::domain_tools::{DomainApi, DomainToolExecutor};
use aura_exec_tools::{ToolCatalog, ToolConfig};
use aura_model::{ModelProvider, ToolDefinition};
use aura_store_db::{RocksStore, SealCipher};
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

pub(super) type Provider = Arc<dyn ModelProvider + Send + Sync>;

impl Node {
    /// Build every service and bundle them into the router state.
    pub(super) fn build_router_state(
        &self,
        store: Arc<RocksStore>,
        seal_cipher: Option<&Arc<SealCipher>>,
        plans: &Arc<PlanResolver>,
    ) -> anyhow::Result<RouterState> {
        let tool_config = self.tool_config();
        let catalog = Arc::new(ToolCatalog::new());
        info!(static_tools = catalog.static_count(), "Tool catalog ready");
        let domain_api = self.domain_api()?;
        let (tools, executors) = core_executors(&catalog, &tool_config, &domain_api);
        let provider = self.model_provider(plans)?;
        let memory_manager = build_memory_manager(&store, seal_cipher)?;

        let scheduler = Arc::new(Scheduler::new(
            store.clone(),
            provider.clone(),
            executors,
            tools,
            self.config.workspaces_path(),
            Some(Arc::clone(&memory_manager)),
        ));
        info!("Scheduler ready");

        let (automaton_bridge, automaton_controller) = build_automaton(
            &store,
            &domain_api,
            &provider,
            &catalog,
            &tool_config,
            &scheduler,
        );
        let skill_manager = self.skill_manager(&store, seal_cipher);
        let router_url = std::env::var("AURA_ROUTER_URL").ok();
        let (secrets_vault, process_store, trigger_registrar) = sealed_stores(&store, seal_cipher);

        Ok(RouterState::new(crate::gateway::RouterStateConfig {
            store,
            scheduler,
            config: self.config.clone(),
            provider,
            tool_config,
            catalog,
            domain_api: Some(domain_api),
            automaton_controller,
            automaton_bridge,
            memory_manager: Some(memory_manager),
            skill_manager: Some(skill_manager),
            secrets_vault: Some(secrets_vault),
            process_store: Some(process_store),
            trigger_registrar: Some(trigger_registrar),
            router_url,
        }))
    }

    fn tool_config(&self) -> ToolConfig {
        let mut tool_config = ToolConfig::for_autonomous_dev_loop();
        if self.config.allow_unrestricted_full_access {
            tool_config.command.allow_unrestricted_full_access = true;
            warn!("unrestricted full-access command allowlist bypass enabled by operator config");
        }
        if tool_config.command.enabled {
            info!(
                allowed_commands = ?tool_config.command.command_allowlist,
                binary_allowlist = ?tool_config.command.binary_allowlist,
                allow_shell = tool_config.command.allow_shell,
                allow_unrestricted_full_access = tool_config.command.allow_unrestricted_full_access,
                "aura-node run_command enabled"
            );
        }
        tool_config
    }

    fn domain_api(&self) -> anyhow::Result<Arc<dyn DomainApi>> {
        let domain_api: Arc<dyn DomainApi> = Arc::new(HttpDomainApi::new(
            &self.config.aura_storage_url,
            &self.config.aura_network_url,
            self.config.aura_os_server_url.clone(),
        )?);
        info!(
            storage_url = %self.config.aura_storage_url,
            os_server_url = ?self.config.aura_os_server_url,
            "Domain API ready (JWT auth)"
        );
        Ok(domain_api)
    }

    fn model_provider(&self, plans: &Arc<PlanResolver>) -> anyhow::Result<Provider> {
        // A cloud node calls its owner's plans (Claude, ChatGPT,
        // OpenRouter) directly, with boxd placeholders its claim brings;
        // everything else still goes to aura-router.
        let provider = if self.config.cloud_node {
            let resolver: Arc<dyn aura_core_auth::CredentialResolver> = plans.clone();
            aura_model::routed_provider(Some(resolver))
                .context("building the cloud node's model provider")?
                .provider
        } else {
            aura_model::default_provider_from_env()
                .context("building default model provider")?
                .provider
        };
        Ok(provider)
    }

    fn skill_manager(
        &self,
        store: &Arc<RocksStore>,
        seal_cipher: Option<&Arc<SealCipher>>,
    ) -> Arc<RwLock<SkillManager>> {
        let mut skill_loader =
            SkillLoader::with_defaults(Some(self.config.workspaces_path()), None);
        // The skills its hub sends (`PUT /v1/cloud/skills/:agent_id`).
        skill_loader
            .config_mut()
            .extra_dirs
            .push(self.config.data_dir.join(crate::gateway::HUB_SKILLS_DIR));
        let skill_install_store = Arc::new(SkillInstallStore::with_cipher(
            store.db_handle().clone(),
            seal_cipher.cloned(),
        ));
        let skill_manager_inner =
            SkillManager::with_install_store(skill_loader, skill_install_store);
        let skill_count = skill_manager_inner.list_all().len();
        let skill_manager = Arc::new(RwLock::new(skill_manager_inner));
        info!(skills = skill_count, "Skill manager ready");
        skill_manager
    }
}

/// The core-profile tool list and the executors that resolve them.
fn core_executors(
    catalog: &Arc<ToolCatalog>,
    tool_config: &ToolConfig,
    domain_api: &Arc<dyn DomainApi>,
) -> (Vec<ToolDefinition>, Vec<Arc<dyn Executor>>) {
    let tools = catalog.visible_tools(ToolProfile::Core, tool_config);
    let domain_exec = Arc::new(DomainToolExecutor::new(domain_api.clone()));
    let resolver =
        aura_engine::executor::build_tool_resolver(catalog, tool_config, Some(domain_exec));
    let resolver: Arc<dyn Executor> = Arc::new(resolver);
    let executors = vec![resolver];
    info!("Executors configured");
    (tools, executors)
}

/// Secrets vault, process store and trigger registrar sharing the
/// node's DB and optional state cipher.
fn sealed_stores(
    store: &Arc<RocksStore>,
    seal_cipher: Option<&Arc<SealCipher>>,
) -> (
    Arc<aura_store_db::SecretsVault>,
    Arc<aura_store_db::ProcessStore>,
    Arc<crate::trigger_registrar::TriggerRegistrar>,
) {
    // In-TEE secrets vault (Swarm TEE phase 6): same shared DB +
    // optional state cipher as the memory/skill stores, so secrets
    // are sealed at rest whenever the node booted in sealed mode.
    let secrets_vault = Arc::new(aura_store_db::SecretsVault::with_cipher(
        store.db_handle().clone(),
        seal_cipher.cloned(),
    ));

    // In-TEE process store (Swarm TEE phase 7): same shared DB +
    // optional state cipher, so process prompts/config and run
    // history are sealed at rest in sealed mode.
    let process_store = Arc::new(aura_store_db::ProcessStore::with_cipher(
        store.db_handle().clone(),
        seal_cipher.cloned(),
    ));

    // Trigger-metadata registrar (Swarm TEE phase 8): pushes the
    // exportable (process_id, cron, enabled, next_run_at) set to
    // the swarm gateway after process mutations. Env-configured;
    // silently inert for local agents.
    let trigger_registrar = Arc::new(crate::trigger_registrar::TriggerRegistrar::from_env(
        process_store.clone(),
    ));
    (secrets_vault, process_store, trigger_registrar)
}
