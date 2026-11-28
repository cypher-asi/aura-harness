//! Where a plan came from, stamped into its frontmatter by `create_plan`:
//! when, by which agent on which model, and on which machine.
//!
//! These are the facts only the writing side knows. Everything the UI can
//! work out for itself (the author's channel, the plan's status, its
//! branch) is left to the UI, so it stays current.

use super::PlanMeta;
use crate::tool::ToolContext;
use chrono::{DateTime, SecondsFormat, Utc};

/// What a machine is called when it is not a cloud machine.
pub const LOCAL_MACHINE: &str = "local";

/// What a cloud machine is called when it has no readable hostname.
const UNNAMED_CLOUD: &str = "cloud";

/// Variable a cloud node sets (see `aura-engine-cloud`'s `machine_env`).
const CLOUD_NODE_ENV: &str = "AURA_CLOUD_NODE";

/// Who, when and where a plan is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanOrigin {
    /// When it is written
    pub at: DateTime<Utc>,
    /// Id (hex) of the agent writing it, when known
    pub author: Option<String>,
    /// Model the author runs on, when known
    pub model: Option<String>,
    /// The machine it is written on (see [`machine_name`])
    pub machine: String,
}

impl PlanOrigin {
    /// Now, on this machine, by an unknown author.
    #[must_use]
    pub fn now() -> Self {
        Self {
            at: Utc::now(),
            author: None,
            model: None,
            machine: machine_name(),
        }
    }

    /// Now, on this machine, by the agent making the tool call `ctx`.
    #[must_use]
    pub fn of_call(ctx: &ToolContext) -> Self {
        Self {
            author: ctx.caller_agent_id.map(|id| id.to_hex()),
            model: ctx
                .caller_model_id
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string),
            ..Self::now()
        }
    }

    /// Stamp `meta`, the plan being written, over `previous`, the plan it
    /// replaces. A new plan takes everything from here. A rewrite keeps
    /// when it was created, its author and its machine (filling in any
    /// it lacks), and is stamped as updated now.
    pub fn stamp(&self, meta: &mut PlanMeta, previous: Option<&PlanMeta>) {
        let at = self.at.to_rfc3339_opts(SecondsFormat::Secs, true);
        let kept =
            |field: fn(&PlanMeta) -> &Option<String>| previous.and_then(|p| field(p).clone());
        meta.created_at = kept(|p| &p.created_at).or_else(|| Some(at.clone()));
        meta.author = kept(|p| &p.author).or_else(|| self.author.clone());
        meta.machine = kept(|p| &p.machine).or_else(|| Some(self.machine.clone()));
        meta.model = self
            .model
            .clone()
            .or_else(|| previous.and_then(|p| p.model.clone()));
        meta.updated_at = previous.map(|_| at);
    }
}

/// This machine as a plan records it: its hostname on a cloud machine,
/// [`LOCAL_MACHINE`] anywhere else.
#[must_use]
pub fn machine_name() -> String {
    let cloud = std::env::var_os(CLOUD_NODE_ENV).is_some_and(|value| !value.is_empty());
    name_machine(cloud, cloud.then(hostname).flatten())
}

/// The machine name for a machine that is `cloud` or not, called
/// `hostname`.
fn name_machine(cloud: bool, hostname: Option<String>) -> String {
    if !cloud {
        return LOCAL_MACHINE.to_string();
    }
    hostname
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| UNNAMED_CLOUD.to_string())
}

/// This machine's hostname, from the kernel, `/etc/hostname` or the
/// environment.
fn hostname() -> Option<String> {
    let from_file = |path: &str| std::fs::read_to_string(path).ok();
    from_file("/proc/sys/kernel/hostname")
        .or_else(|| from_file("/etc/hostname"))
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_is_local_unless_a_cloud_node() {
        assert_eq!(name_machine(false, Some("box".into())), "local");
        assert_eq!(name_machine(false, None), "local");
    }

    #[test]
    fn cloud_machine_is_its_hostname_or_cloud() {
        assert_eq!(
            name_machine(true, Some("aura-rus199-12b4\n".into())),
            "aura-rus199-12b4"
        );
        assert_eq!(name_machine(true, Some("  ".into())), "cloud");
        assert_eq!(name_machine(true, None), "cloud");
    }
}
