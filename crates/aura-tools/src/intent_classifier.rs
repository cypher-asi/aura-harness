//! Intent classifier: keyword-driven per-turn tool filter.
//!
//! Phase 2 of the super-agent / harness unification plan ports the CEO
//! super-agent's tier-1/tier-2 classifier from
//! `aura-os-super-agent-profile::classify_intent` into the harness as a
//! reusable data-only primitive.
//!
//! At session boot the harness deserializes a portable
//! [`SuperAgentProfile`]-shaped JSON blob (or any subset matching
//! [`IntentClassifier::from_profile_json`]) and uses the resulting
//! classifier to filter the exposed tool list each turn, so the model
//! only sees the tools relevant to the current user message.
//!
//! The classifier is intentionally:
//! - **pure** (no I/O, no async, no dependencies on service types),
//! - **data-driven** (rules and tier-1 list come from JSON), and
//! - **stateless per-session** (cheap to construct per request).
//!
//! Integrating this as a pre-turn hook into
//! [`aura_agent::AgentLoop`][al] happens in phase 3; phase 2.3 only
//! ships the primitive and a cross-repo parity test.
//!
//! [al]: ../../aura-agent/src/agent_loop/index.html

use serde::{Deserialize, Serialize};

use aura_core::ToolDefinition;

/// A single keyword → domain rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProfileRule {
    domain: String,
    keywords: Vec<String>,
}

/// Minimal subset of the aura-os-super-agent profile JSON that the
/// classifier needs. Deliberately kept local so the harness does not
/// pull in `aura-os-core`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProfileView {
    tier1_domains: Vec<String>,
    classifier_rules: Vec<ProfileRule>,
}

/// Errors from [`IntentClassifier::from_profile_json`].
#[derive(Debug, thiserror::Error)]
pub enum ClassifierError {
    #[error("invalid profile JSON: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Per-turn tool filter driven by keyword rules.
///
/// Construct via [`Self::from_profile_json`] or
/// [`Self::from_rules`]; use [`Self::visible_domains`] for the raw
/// classification and [`Self::filter_tools`] to prune a
/// [`Vec<ToolDefinition>`] against a tool-name → domain manifest.
#[derive(Debug, Clone)]
pub struct IntentClassifier {
    tier1: Vec<String>,
    rules: Vec<(String, Vec<String>)>,
}

impl IntentClassifier {
    /// Build a classifier from the portable super-agent profile JSON.
    ///
    /// Accepts either a full [`SuperAgentProfile`] JSON (from
    /// `aura-os-super-agent-profile`) or any JSON exposing
    /// `tier1_domains` + `classifier_rules`. `ToolDomain` enum values
    /// deserialize as `snake_case` strings ("billing", "process", ...)
    /// so the harness never has to know the aura-os enum.
    pub fn from_profile_json(value: &serde_json::Value) -> Result<Self, ClassifierError> {
        let view: ProfileView = serde_json::from_value(value.clone())?;
        Ok(Self::from_view(view))
    }

    /// Build a classifier directly from rules. `tier1` domains are
    /// always present in the output set; `rules` is a list of
    /// `(domain, keywords)` tuples where any keyword match adds the
    /// domain.
    #[must_use]
    pub fn from_rules(
        tier1: Vec<String>,
        rules: Vec<(String, Vec<String>)>,
    ) -> Self {
        Self { tier1, rules }
    }

    fn from_view(view: ProfileView) -> Self {
        Self {
            tier1: view.tier1_domains,
            rules: view
                .classifier_rules
                .into_iter()
                .map(|r| (r.domain, r.keywords))
                .collect(),
        }
    }

    /// Domains visible for `message`: all tier-1 + any tier-2 triggered
    /// by keyword match. Matches
    /// `aura_os_super_agent_profile::classify_intent` byte-for-byte
    /// when constructed from the same profile.
    #[must_use]
    pub fn visible_domains(&self, message: &str) -> Vec<String> {
        let lower = message.to_lowercase();
        let mut out: Vec<String> = self.tier1.clone();
        for (domain, keywords) in &self.rules {
            if keywords.iter().any(|k| lower.contains(k)) {
                out.push(domain.clone());
            }
        }
        out.dedup();
        out
    }

    /// Filter a set of tool definitions to only the ones whose domain
    /// is visible this turn.
    ///
    /// `manifest` is a `(tool_name, domain_name)` map. Tools whose name
    /// is not in the manifest are passed through unchanged (so core
    /// filesystem / shell tools stay visible regardless of classifier
    /// state).
    #[must_use]
    pub fn filter_tools(
        &self,
        message: &str,
        manifest: &[(String, String)],
        tools: &[ToolDefinition],
    ) -> Vec<ToolDefinition> {
        let visible = self.visible_domains(message);
        tools
            .iter()
            .filter(|t| match manifest.iter().find(|(n, _)| n == &t.name) {
                Some((_, domain)) => visible.iter().any(|d| d == domain),
                None => true,
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ceo_profile_fixture() -> serde_json::Value {
        // Mirrors aura-os-super-agent-profile::SuperAgentProfile::ceo_default()
        // for the fields the classifier consumes. Kept in-file so the
        // harness build does not depend on the aura-os crates.
        json!({
            "tier1_domains": ["project", "agent", "execution", "monitoring"],
            "classifier_rules": [
                {"domain": "org",        "keywords": ["org", "organization", "team", "member", "invite"]},
                {"domain": "billing",    "keywords": ["bill", "credit", "balance", "cost", "pay", "checkout", "purchase"]},
                {"domain": "social",     "keywords": ["feed", "post", "comment", "follow", "social"]},
                {"domain": "task",       "keywords": ["task", "extract", "transition", "retry", "run task"]},
                {"domain": "spec",       "keywords": ["spec", "specification", "requirements", "generate spec"]},
                {"domain": "system",     "keywords": ["file", "browse", "directory", "system info", "environment", "remote", "vm"]},
                {"domain": "generation", "keywords": ["image", "generate image", "3d", "model", "render", "logo"]},
                {"domain": "process",    "keywords": [
                    "process", "workflow", "node", "ignition", "pipeline",
                    "automate", "trigger", "cron", "schedule", "scheduled",
                    "recurring", "every day", "every hour", "every morning",
                    "daily", "weekly", "periodic"
                ]}
            ]
        })
    }

    #[test]
    fn tier1_always_present() {
        let c = IntentClassifier::from_profile_json(&ceo_profile_fixture()).unwrap();
        let d = c.visible_domains("");
        assert!(d.contains(&"project".to_string()));
        assert!(d.contains(&"agent".to_string()));
        assert!(d.contains(&"execution".to_string()));
        assert!(d.contains(&"monitoring".to_string()));
    }

    #[test]
    fn schedule_language_loads_process_domain() {
        let c = IntentClassifier::from_profile_json(&ceo_profile_fixture()).unwrap();
        let d = c.visible_domains("Create a recurring daily process schedule");
        assert!(d.contains(&"process".to_string()));
    }

    #[test]
    fn billing_keywords_add_billing_domain() {
        let c = IntentClassifier::from_profile_json(&ceo_profile_fixture()).unwrap();
        assert!(c
            .visible_domains("check my credit balance")
            .contains(&"billing".to_string()));
        assert!(c
            .visible_domains("purchase more credits")
            .contains(&"billing".to_string()));
    }

    #[test]
    fn case_insensitive_matching() {
        let c = IntentClassifier::from_profile_json(&ceo_profile_fixture()).unwrap();
        assert!(c
            .visible_domains("INVITE A TEAM MEMBER")
            .contains(&"org".to_string()));
    }

    #[test]
    fn dedupes_multiple_hits_of_same_domain() {
        let c = IntentClassifier::from_profile_json(&ceo_profile_fixture()).unwrap();
        let d = c.visible_domains("post a comment to the feed");
        let count = d.iter().filter(|s| s.as_str() == "social").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn filter_tools_keeps_tier1_drops_unmatched_tier2() {
        let c = IntentClassifier::from_profile_json(&ceo_profile_fixture()).unwrap();

        let manifest = vec![
            ("create_project".to_string(), "project".to_string()),
            ("list_specs".to_string(), "spec".to_string()),
            ("get_credit_balance".to_string(), "billing".to_string()),
        ];
        let tools = vec![
            ToolDefinition::new("create_project", "create", json!({})),
            ToolDefinition::new("list_specs", "specs", json!({})),
            ToolDefinition::new("get_credit_balance", "$", json!({})),
            ToolDefinition::new("read_file", "fs", json!({})), // not in manifest; stays
        ];

        // Only tier-1 domains; no tier-2 keywords present.
        let out = c.filter_tools("hello", &manifest, &tools);
        let names: Vec<&str> = out.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"create_project"));
        assert!(names.contains(&"read_file"));
        assert!(!names.contains(&"list_specs"));
        assert!(!names.contains(&"get_credit_balance"));

        // Trigger billing + spec domains.
        let out = c.filter_tools("draft a spec and pay me", &manifest, &tools);
        let names: Vec<&str> = out.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"create_project"));
        assert!(names.contains(&"list_specs"));
        assert!(names.contains(&"get_credit_balance"));
        assert!(names.contains(&"read_file"));
    }

    #[test]
    fn parity_with_aura_os_super_agent_classify_intent() {
        // Known inputs from aura-os-super-agent-profile tests;
        // harness-side classifier must produce the same domain sets
        // when fed the matching profile.
        let c = IntentClassifier::from_profile_json(&ceo_profile_fixture()).unwrap();

        let cases: &[(&str, &[&str])] = &[
            ("", &["project", "agent", "execution", "monitoring"]),
            (
                "Create a recurring daily process schedule",
                &["project", "agent", "execution", "monitoring", "process"],
            ),
            (
                "invite a team member",
                &["project", "agent", "execution", "monitoring", "org"],
            ),
            (
                "check my credit balance",
                &["project", "agent", "execution", "monitoring", "billing"],
            ),
        ];

        for (msg, expected) in cases {
            let actual = c.visible_domains(msg);
            let actual_set: std::collections::BTreeSet<_> = actual.iter().cloned().collect();
            let expected_set: std::collections::BTreeSet<_> =
                expected.iter().map(|s| (*s).to_string()).collect();
            assert_eq!(
                actual_set, expected_set,
                "domain set mismatch for message {msg:?}"
            );
        }
    }

    #[test]
    fn rejects_malformed_profile_json() {
        let bad = json!({"nope": true});
        assert!(IntentClassifier::from_profile_json(&bad).is_err());
    }

    #[test]
    fn from_rules_constructor_works_without_json() {
        let c = IntentClassifier::from_rules(
            vec!["project".to_string()],
            vec![("billing".to_string(), vec!["credit".to_string()])],
        );
        let d = c.visible_domains("show me my credits");
        assert!(d.contains(&"project".to_string()));
        assert!(d.contains(&"billing".to_string()));
    }
}
