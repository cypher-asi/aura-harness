//! The integrations aura ships with: [`GitHub`] and [`Boxd`].
//!
//! Both only describe and check what aura already uses; how GitHub and
//! boxd work today (the `gh` CLI or `GH_TOKEN` for the REPO tab, the
//! GitHub App for cloud pushes, `BOXD_API_KEY` from `.env` for cloud
//! machines) is unchanged. They add a status, a Test, and a sealed place
//! for a token or key entered in the Integrations panel.

mod boxd;
mod github;

pub use self::boxd::Boxd;
pub use self::github::GitHub;

use crate::integration::Integration;
use std::sync::Arc;

/// What the built-in integrations need to know about this hub.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuiltinOptions {
    /// Whether the hub has a GitHub App configured for cloud pushes
    /// (`AURA_GITHUB_APP_*`)
    pub github_app: bool,
}

/// Every built-in integration, in the panel's order.
#[must_use]
pub fn builtin(options: BuiltinOptions) -> Vec<Arc<dyn Integration>> {
    vec![
        Arc::new(GitHub::new(options.github_app)),
        Arc::new(Boxd::new()),
    ]
}
