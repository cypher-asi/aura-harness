//! HTTP-backed `DomainApi` implementation.
//!
//! All routes use `Authorization: Bearer <jwt>` (user JWT from session).
//!
//! Phase C / Commit 4 relocates this from `aura-node` into the
//! standalone `aura-exec-domain-http` crate so the gateway no longer owns
//! domain HTTP routing or depends on `reqwest` for domain calls.

use anyhow::Context;
use reqwest::Client;

mod domain_api;
mod request;

pub struct HttpDomainApi {
    http: Client,
    storage_url: String,
    network_url: String,
    /// Optional `aura-os-server` base URL. When set, it replaces
    /// [`Self::storage_url`] as the base for spec / task / project /
    /// log routes so those writes hit `aura-os-server` and fire its
    /// side effects (disk mirror of spec markdown to
    /// `<workspace_root>/spec/<slug>.md`, SSE broadcast on the project
    /// stream, JWT billing header injection). Feed / billing routes
    /// are unaffected — they keep using their existing direct base
    /// URLs because `aura-os-server` does not proxy them.
    ///
    /// Sourced from `NodeConfig::aura_os_server_url` (env var
    /// `AURA_OS_SERVER_URL`). `None` preserves the historical
    /// behavior of posting directly to `aura-storage`.
    os_server_url: Option<String>,
}

impl HttpDomainApi {
    /// Build a new HTTP-backed domain API.
    ///
    /// `os_server_url` is the optional `aura-os-server` base URL. When
    /// `Some`, spec / task / project / log routes go through it so
    /// `aura-os-server`'s disk-mirror + SSE + JWT-billing side effects
    /// fire on every write. When `None` those routes fall back to
    /// `storage_url` (pre-`aura-os-server` behavior). Operators enable
    /// the override by setting `AURA_OS_SERVER_URL` on the node.
    ///
    /// # Errors
    /// Returns an error if `reqwest` fails to construct its HTTP client
    /// (typically a TLS backend initialization failure in constrained
    /// environments).
    pub fn new(
        storage_url: &str,
        network_url: &str,
        os_server_url: Option<String>,
    ) -> anyhow::Result<Self> {
        let http = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("build HTTP client for HttpDomainApi")?;
        Ok(Self {
            http,
            storage_url: storage_url.trim_end_matches('/').to_string(),
            network_url: network_url.trim_end_matches('/').to_string(),
            os_server_url: os_server_url.map(|u| u.trim_end_matches('/').to_string()),
        })
    }

    /// Base URL for routes `aura-os-server` owns (specs, tasks,
    /// projects, logs, project stats).
    ///
    /// Returns `os_server_url` when configured, otherwise falls back
    /// to `storage_url`. Kept as a helper so every spec/task/log
    /// handler funnels through a single override point — threading the
    /// new URL is a one-line change per call site and an operator who
    /// leaves `AURA_OS_SERVER_URL` unset keeps the historical direct
    /// `aura-storage` path verbatim.
    fn specs_tasks_base_url(&self) -> &str {
        self.os_server_url.as_deref().unwrap_or(&self.storage_url)
    }

    /// Base URL for project GET / PUT routes.
    ///
    /// Unlike specs / tasks / logs / stats, `/api/projects/:id` is not
    /// a route aura-storage exposes — it lives on aura-network (and,
    /// when the override is set, aura-os-server). Routing the fallback
    /// through `storage_url` therefore 404s every `get_project` call
    /// the dev-loop makes before it can run a single task. Keep this
    /// helper separate from `specs_tasks_base_url` so the project path
    /// lands on `network_url` when `AURA_OS_SERVER_URL` is unset, which
    /// is the pre-`feat(domain): route DomainApi writes through
    /// aura-os-server` behavior that every prior operator setup relied
    /// on.
    fn project_base_url(&self) -> &str {
        self.os_server_url.as_deref().unwrap_or(&self.network_url)
    }
}

#[cfg(test)]
mod tests;
