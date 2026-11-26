use std::path::PathBuf;

use crate::ids::{AgentId, Hash};
use crate::types::compare::{
    ArmSpec, BenchSource, CompareArm, CompareCopy, CompareJudgment, ComparePayload, CompareRun,
    CompareStatus, ModelTokens, RunTurn,
};
use crate::types::gate::{Difficulty, OutcomeSignal, PromptCategory};
use crate::types::plan_usage::PlanShare;
use crate::types::transaction::SystemKind;

pub(super) fn payload() -> ComparePayload {
    ComparePayload {
        system_kind: SystemKind::Compare,
        task: "fix the parser".to_string(),
        plan: None,
        arm_a: ArmSpec::Gate { delegate: true },
        arm_b: ArmSpec::Model {
            model: "claude/claude-opus-4-7".to_string(),
        },
        against: "claude/claude-opus-4-7".to_string(),
        prompts: vec!["fix the parser".to_string()],
        source: BenchSource::Prompt,
        repeats: 3,
        check: Some("cargo test".to_string()),
        budget_usd: Some(2.5),
        classifier: "claude/claude-haiku-4-5".to_string(),
        classify_tokens: Vec::new(),
        requested_by: AgentId::generate(),
        apply_root: PathBuf::from("/work/repo"),
        started_ms: 1_000,
        runs: vec![run()],
        judgments: vec![judgment()],
        notes: Vec::new(),
        status: CompareStatus::Applied("a-1".to_string()),
    }
}

fn run() -> CompareRun {
    CompareRun {
        name: "a-1".to_string(),
        arm: CompareArm::A,
        agent_id: AgentId::generate(),
        prompt_tx: Some(Hash::from_content(b"prompt")),
        model: "claude/claude-sonnet-4-6".to_string(),
        copy: CompareCopy {
            root: PathBuf::from("/home/.aura/compare/c1/a-1"),
            workdir: PathBuf::from("/home/.aura/compare/c1/a-1/app"),
            git_dir: None,
            baseline: "abc123".to_string(),
        },
        wall_ms: 42_000,
        cost_usd: 0.12,
        tokens: vec![ModelTokens {
            model: "claude-sonnet-4-6".to_string(),
            calls: 3,
            input_tokens: 30_000,
            output_tokens: 2_000,
            cost_usd: 0.12,
            ..ModelTokens::default()
        }],
        turns: vec![turn()],
        plan_usage: vec![PlanShare {
            plan: crate::Plan::Claude,
            window: "5h".to_string(),
            first: 0.10,
            last: 0.12,
            used: 0.03,
        }],
        check_passed: Some(true),
        files_changed: 2,
        error: None,
        task: None,
    }
}

fn turn() -> RunTurn {
    RunTurn {
        category: Some(PromptCategory::Debugging),
        difficulty: Some(Difficulty::Hard),
        led: Some(false),
        escalations: 1,
        first_model: Some("claude/claude-haiku-4-5".to_string()),
        effort: Some("max".to_string()),
        first_effort: Some("high".to_string()),
        nudges: 1,
        sent_back: 1,
        ..RunTurn::new(
            "claude/claude-sonnet-4-6",
            0.12,
            42_000,
            OutcomeSignal::ChecksPassed,
        )
    }
}

fn judgment() -> CompareJudgment {
    CompareJudgment {
        a_run: "a-1".to_string(),
        b_run: "b-1".to_string(),
        a_first: false,
        winner: None,
        parsed: true,
        a_vs_reference: None,
        b_vs_reference: None,
        model: "claude/claude-opus-4-7".to_string(),
        cost_usd: 0.03,
        unpriced_calls: 0,
        reasoning: "Both are fine.".to_string(),
    }
}

pub(super) fn by_difficulty() -> ArmSpec {
    ArmSpec::ByDifficulty {
        easy: "claude/claude-haiku-4-5".to_string(),
        medium: "openrouter/deepseek/deepseek-v3.2".to_string(),
        hard: "chatgpt/gpt-5.4".to_string(),
    }
}
