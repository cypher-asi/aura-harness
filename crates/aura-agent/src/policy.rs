//! Turn-configuration policy: heuristics that determine how the agent loop
//! runs for a given task (complexity classification, token budgets, exploration
//! allowances, model selection).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskComplexity {
    Simple,
    Standard,
    Complex,
}

pub fn classify_task_complexity(title: &str, description: &str) -> TaskComplexity {
    let combined = format!("{} {}", title, description).to_lowercase();
    let mut score: i32 = 0;

    let simple_signals: &[(&str, i32)] = &[
        ("add dependency", -3),
        ("add dep ", -3),
        ("set up dependency", -3),
        ("define enum", -2),
        ("define struct", -2),
        ("define type", -2),
        ("add import", -2),
        ("update cargo.toml", -2),
        ("update package.json", -2),
        ("rename ", -1),
        ("move file", -1),
    ];
    let complex_signals: &[(&str, i32)] = &[
        ("integration test", 3),
        ("end-to-end", 3),
        ("e2e test", 3),
        ("refactor", 3),
        ("migrate", 3),
        ("rewrite", 3),
        ("multi-file", 2),
        ("cross-crate", 2),
        ("implement service", 3),
        ("implement api", 3),
    ];

    for &(pattern, weight) in simple_signals {
        if combined.contains(pattern) {
            score += weight;
        }
    }
    for &(pattern, weight) in complex_signals {
        if combined.contains(pattern) {
            score += weight;
        }
    }

    if description.len() > 1000 {
        score += 2;
    } else if description.len() < 200 {
        score -= 1;
    }

    if score <= -2 {
        TaskComplexity::Simple
    } else if score >= 2 {
        TaskComplexity::Complex
    } else {
        TaskComplexity::Standard
    }
}

pub fn compute_thinking_budget(base: u32, member_count: usize) -> u32 {
    if member_count >= 15 {
        base.max(16_000)
    } else if member_count >= 8 {
        base.max(10_000)
    } else {
        base
    }
}

pub fn compute_exploration_allowance(
    task_title: &str,
    task_description: &str,
    member_count: usize,
) -> usize {
    let complexity = classify_task_complexity(task_title, task_description);
    let combined = format!("{} {}", task_title, task_description).to_lowercase();

    let is_refactoring = combined.contains("refactor")
        || combined.contains("rename across")
        || combined.contains("migrate")
        || combined.contains("multi-file");

    let base: usize = match complexity {
        TaskComplexity::Simple => 8,
        TaskComplexity::Standard => 12,
        TaskComplexity::Complex => {
            if is_refactoring {
                22
            } else {
                18
            }
        }
    };

    if member_count >= 15 {
        base + 4
    } else if member_count >= 8 {
        base + 2
    } else {
        base
    }
}

/// Returns the model to use for simple tasks. Checks the `AURA_SIMPLE_MODEL`
/// env var first, falling back to `default_model`.
pub fn resolve_simple_model(default_model: &str) -> String {
    std::env::var("AURA_SIMPLE_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_model.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_task_complexity_simple_patterns() {
        assert_eq!(
            classify_task_complexity("Add dependency for serde", ""),
            TaskComplexity::Simple
        );
        assert_eq!(
            classify_task_complexity("Define enum Status", ""),
            TaskComplexity::Simple
        );
        assert_eq!(
            classify_task_complexity("Rename the module", "short"),
            TaskComplexity::Simple
        );
        assert_eq!(
            classify_task_complexity("Update Cargo.toml", ""),
            TaskComplexity::Simple
        );
    }

    #[test]
    fn classify_task_complexity_complex_patterns() {
        assert_eq!(
            classify_task_complexity("Refactor auth module", ""),
            TaskComplexity::Complex
        );
        assert_eq!(
            classify_task_complexity("Add integration test for API", ""),
            TaskComplexity::Complex
        );
        assert_eq!(
            classify_task_complexity("Implement service layer", ""),
            TaskComplexity::Complex
        );
        assert_eq!(
            classify_task_complexity("Migrate to new storage", ""),
            TaskComplexity::Complex
        );
    }

    #[test]
    fn classify_task_complexity_standard_for_moderate_descriptions() {
        let desc = "a".repeat(500);
        assert_eq!(
            classify_task_complexity("Add handler", &desc),
            TaskComplexity::Standard
        );
    }

    #[test]
    fn classify_task_complexity_long_desc_is_complex() {
        let desc = "a".repeat(1500);
        assert_eq!(
            classify_task_complexity("Add handler", &desc),
            TaskComplexity::Complex
        );
    }

    #[test]
    fn compute_thinking_budget_base_for_small_workspace() {
        assert_eq!(compute_thinking_budget(8000, 3), 8000);
    }

    #[test]
    fn compute_thinking_budget_scales_for_medium_workspace() {
        assert_eq!(compute_thinking_budget(8000, 10), 10_000);
    }

    #[test]
    fn compute_thinking_budget_scales_for_large_workspace() {
        assert_eq!(compute_thinking_budget(8000, 20), 16_000);
    }

    #[test]
    fn compute_exploration_allowance_simple_small_workspace() {
        assert_eq!(
            compute_exploration_allowance("Add dependency for serde", "", 3),
            8
        );
    }

    #[test]
    fn compute_exploration_allowance_complex_refactoring_large_workspace() {
        assert_eq!(
            compute_exploration_allowance("Refactor the auth module", "", 20),
            26
        );
    }

    #[test]
    fn compute_exploration_allowance_standard_medium_workspace() {
        let desc = "a".repeat(500);
        assert_eq!(compute_exploration_allowance("Add handler", &desc, 10), 14);
    }

    #[test]
    fn resolve_simple_model_uses_default_when_no_env() {
        // Clear the env var in case it's set
        std::env::remove_var("AURA_SIMPLE_MODEL");
        assert_eq!(resolve_simple_model("test-model"), "test-model");
    }

    #[test]
    fn resolve_simple_model_uses_env_when_set() {
        std::env::set_var("AURA_SIMPLE_MODEL", "custom-model");
        assert_eq!(resolve_simple_model("test-model"), "custom-model");
        std::env::remove_var("AURA_SIMPLE_MODEL");
    }
}
