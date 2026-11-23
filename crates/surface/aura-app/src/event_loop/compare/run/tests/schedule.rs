//! What the user's chat hears as each run ends.

use super::super::schedule::progress;
use aura_core_types::{AgentId, CompareArm, CompareCopy, CompareRun};
use std::path::PathBuf;

/// Run `a-1` as it ended: in 3m 05s, at $0.004, its check as
/// `check_passed` says, and stopped by `error` when there is one.
fn ended(check_passed: Option<bool>, error: Option<&str>) -> CompareRun {
    CompareRun {
        name: "a-1".to_string(),
        arm: CompareArm::A,
        agent_id: AgentId::new([1; 32]),
        prompt_tx: None,
        model: "aura-claude-sonnet-4-6".to_string(),
        copy: CompareCopy {
            root: PathBuf::new(),
            workdir: PathBuf::new(),
            git_dir: None,
            baseline: String::new(),
        },
        wall_ms: 185_000,
        cost_usd: 0.004,
        tokens: Vec::new(),
        plan_usage: Vec::new(),
        turns: Vec::new(),
        check_passed,
        files_changed: 0,
        error: error.map(str::to_string),
        task: None,
    }
}

#[test]
fn the_chat_hears_how_each_run_ended() {
    let stopped = Some("opening the run: no session");
    for (check, error, line) in [
        (
            Some(true),
            None,
            "Compare #1: a-1 done in 3m 05s, $0.0040, check passed",
        ),
        (
            Some(false),
            None,
            "Compare #1: a-1 done in 3m 05s, $0.0040, check failed",
        ),
        (None, None, "Compare #1: a-1 done in 3m 05s, $0.0040"),
        (
            None,
            stopped,
            "Compare #1: a-1 stopped: opening the run: no session",
        ),
    ] {
        assert_eq!(progress("Compare #1", &ended(check, error)), line);
    }
}
