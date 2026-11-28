use super::*;

const fn end(wrote_plan: Option<&str>) -> TurnEnd<'_> {
    TurnEnd {
        ok: true,
        wrote_plan,
        shipped: false,
    }
}

/// A flow whose task's work started.
fn planning() -> (Flows, AgentId) {
    let flows = Flows::default();
    let agent = AgentId::generate();
    flows.assign(agent);
    assert!(flows.begin(agent));
    (flows, agent)
}

#[test]
fn a_written_plan_is_built_next_and_the_answer_waits() {
    let (flows, agent) = planning();
    let next = flows.turn_ended(agent, end(Some(".aura/plans/x.plan.md")));
    assert_eq!(next, Next::Build(".aura/plans/x.plan.md".to_string()));
    assert!(next.defers_answer());
    assert_eq!(flows.stage(agent), Some(Stage::Building));
}

#[test]
fn a_built_plan_that_was_not_pushed_is_shipped_then_the_flow_ends() {
    let (flows, agent) = planning();
    flows.turn_ended(agent, end(Some("p")));
    let next = flows.turn_ended(agent, end(None));
    assert_eq!(next, Next::AskToShip);
    assert!(next.defers_answer());
    assert_eq!(flows.turn_ended(agent, end(None)), Next::Normal);
    assert_eq!(flows.stage(agent), None);
}

#[test]
fn a_build_that_pushed_ends_the_flow() {
    let shipped = TurnEnd {
        shipped: true,
        ..end(None)
    };
    assert_eq!(
        advance(Some(Stage::Building), shipped),
        (None, Next::Normal)
    );
}

#[test]
fn a_turn_that_asked_instead_of_planning_keeps_planning() {
    let (flows, agent) = planning();
    assert_eq!(flows.turn_ended(agent, end(None)), Next::Normal);
    assert!(flows.begin(agent), "the reply comes back to planning");
    assert_eq!(
        flows.turn_ended(agent, end(Some("p"))),
        Next::Build("p".to_string())
    );
}

#[test]
fn rewriting_the_same_plan_file_counts_as_a_written_plan() {
    let (flows, agent) = planning();
    // The agent's old plan is at the same path; it ran create_plan again.
    assert_eq!(
        flows.turn_ended(agent, end(Some(".aura/plans/task.plan.md"))),
        Next::Build(".aura/plans/task.plan.md".to_string())
    );
}

#[test]
fn a_task_for_a_busy_agent_plans_only_once_its_work_starts() {
    let flows = Flows::default();
    let agent = AgentId::generate();
    flows.assign(agent);
    // The turn running when it came ends, even having written a plan.
    assert_eq!(flows.turn_ended(agent, end(Some("old"))), Next::Normal);
    assert_eq!(flows.stage(agent), Some(Stage::Assigned));
    assert!(flows.begin(agent));
    assert_eq!(flows.stage(agent), Some(Stage::Planning));
}

#[test]
fn a_failed_planning_turn_leaves_plan_mode_and_others_just_end() {
    let failed = TurnEnd {
        ok: false,
        ..end(Some("p"))
    };
    assert_eq!(
        advance(Some(Stage::Planning), failed),
        (None, Next::LeftPlanning)
    );
    for stage in [Stage::Building, Stage::Shipping] {
        assert_eq!(advance(Some(stage), failed), (None, Next::Normal));
    }
}

#[test]
fn agents_outside_the_flow_are_left_alone() {
    let flows = Flows::default();
    let agent = AgentId::generate();
    assert!(!flows.begin(agent));
    assert_eq!(advance(None, end(Some("p"))), (None, Next::Normal));
}

#[test]
fn leaving_ends_the_flow() {
    let (flows, agent) = planning();
    flows.turn_ended(agent, end(Some("p")));
    flows.leave(agent);
    assert_eq!(flows.stage(agent), None);
}
