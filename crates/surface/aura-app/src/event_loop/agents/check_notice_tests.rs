use super::*;
use aura_agent::CheckCommand;

fn report(source: CheckSource, outcome: CheckOutcome) -> CheckReport {
    CheckReport {
        command: CheckCommand {
            command: "cargo check".into(),
            source,
        },
        outcome,
    }
}

fn text(command: Option<UiCommand>) -> Option<String> {
    match command? {
        UiCommand::ShowWarning(t) | UiCommand::ShowSuccess(t) => Some(t),
        _ => None,
    }
}

#[test]
fn failures_name_the_command_and_its_origin() {
    let failed = CheckOutcome::Failed {
        exit_code: Some(101),
        timed_out: false,
    };
    assert_eq!(
        text(check_notice(&report(CheckSource::AgentsMd, failed), false)).as_deref(),
        Some(
            "Check `cargo check` (from AGENTS.md) failed with exit code 101; the agent was \
             given the output"
        )
    );
}

#[test]
fn quiet_while_nothing_changes() {
    let pass = CheckOutcome::Passed { recovered: false };
    assert!(check_notice(&report(CheckSource::Env, pass.clone()), true).is_some());
    assert!(check_notice(&report(CheckSource::Env, pass), false).is_none());
    assert!(check_notice(&report(CheckSource::Env, CheckOutcome::StillFailing), false).is_none());
    let recovered = CheckOutcome::Passed { recovered: true };
    assert_eq!(
        text(check_notice(&report(CheckSource::Env, recovered), false)).as_deref(),
        Some("Check `cargo check` passes again")
    );
}
