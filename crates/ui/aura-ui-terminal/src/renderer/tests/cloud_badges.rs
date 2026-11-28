//! A cloud agent shows its cloud in every list it is in, whatever it is
//! doing: idle, starting, working, waiting for its machine, failed, in
//! plan mode, with its machine asleep or just woken. A cloud member plans
//! first, so plan mode is the state the cloud went missing in.

use super::channels::channels_app;
use super::*;
use crate::events::{AgentSummary, UiCommand};
use crate::host::{PlacementState, PlacementView};

/// What a cloud agent can be doing, as its row is told.
#[derive(Debug, Clone, Copy)]
enum Doing {
    Idle,
    Starting,
    Working,
    Waiting,
    Failed,
    Planning,
    Asleep,
}

const EVERY: [Doing; 7] = [
    Doing::Idle,
    Doing::Starting,
    Doing::Working,
    Doing::Waiting,
    Doing::Failed,
    Doing::Planning,
    Doing::Asleep,
];

/// `row` as its list is told when the agent does `doing`.
fn doing(row: AgentSummary, doing: Doing) -> AgentSummary {
    match doing {
        Doing::Idle | Doing::Asleep => row,
        Doing::Starting => AgentSummary {
            starting: true,
            ..row
        },
        Doing::Working => AgentSummary {
            running: true,
            ..row
        },
        Doing::Waiting => AgentSummary {
            running: true,
            waiting: true,
            ..row
        },
        Doing::Failed => AgentSummary {
            error: Some("cloud turn failed".to_string()),
            ..row
        },
        Doing::Planning => AgentSummary {
            running: true,
            plan_mode: true,
            ..row
        },
    }
}

/// `agent_id` placed in the cloud, its machine asleep when `doing` is.
fn place(app: &mut crate::App, agent_id: &str, doing: Doing) {
    app.process_command(UiCommand::SetPlacement {
        agent_id: agent_id.to_string(),
        state: PlacementState {
            placement: PlacementView::Cloud,
            asleep: matches!(doing, Doing::Asleep),
            ..PlacementState::default()
        },
    });
}

/// The first screen row holding `needle`.
fn row<'a>(rows: &'a [String], needle: &str) -> &'a str {
    rows.iter()
        .find(|row| row.contains(needle))
        .unwrap_or_else(|| panic!("no {needle:?} in:\n{}", rows.join("\n")))
}

#[test]
fn a_cloud_agent_and_its_subagent_show_their_cloud_on_projects_whatever_they_do() {
    for state in EVERY {
        let mut app = crate::App::new();
        let agent = |id: &str, name: &str, depth| AgentSummary {
            id: id.to_string(),
            name: name.to_string(),
            depth,
            ..AgentSummary::default()
        };
        app.process_command(UiCommand::SetAgents(vec![
            agent("lead", "Lead", 0),
            doing(agent("rus", "Rustacean", 0), state),
            doing(agent("sub", "Subtask", 1), state),
            agent("here", "Local", 0),
        ]));
        app.process_command(UiCommand::SetActiveAgent("lead".to_string()));
        place(&mut app, "rus", state);
        place(&mut app, "sub", state);
        let rows = render_rows(&mut app, 140);
        for name in ["Rustacean", "Subtask"] {
            assert!(
                row(&rows, name).contains('☁'),
                "{state:?}: {}",
                row(&rows, name)
            );
        }
        assert!(!row(&rows, "Local").contains('☁'), "a local agent has none");
    }
}

#[test]
fn a_cloud_member_shows_its_cloud_whatever_it_does() {
    for state in EVERY {
        let mut app = channels_app();
        let mut channels = app.channels().to_vec();
        channels[0].members = vec![
            AgentSummary {
                id: "co".to_string(),
                name: "Coordinator".to_string(),
                definition: Some("coordinator".to_string()),
                code: Some("CO".to_string()),
                ..AgentSummary::default()
            },
            doing(
                AgentSummary {
                    id: "r1".to_string(),
                    name: "RUS2".to_string(),
                    definition: Some("rust-engineer".to_string()),
                    code: Some("RUS2".to_string()),
                    title: Some("Why members stall".to_string()),
                    ..AgentSummary::default()
                },
                state,
            ),
        ];
        app.process_command(UiCommand::SetChannels(channels));
        app.open_channel_by_id("c1");
        place(&mut app, "r1", state);
        let rows = render_rows(&mut app, 140);
        let member = row(&rows, "RUS2");
        assert!(member.contains("rust-engineer"), "{state:?}: {member}");
        assert!(member.contains('☁'), "{state:?}: {member}");
    }
}

#[test]
fn a_planning_cloud_agent_shows_its_plan_badge_then_its_cloud() {
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![doing(
        AgentSummary {
            id: "rus".to_string(),
            name: "Rustacean".to_string(),
            ..AgentSummary::default()
        },
        Doing::Planning,
    )]));
    place(&mut app, "rus", Doing::Planning);
    let buffer = render_buffer(&mut app, 140);
    let at = row_of(&buffer, "Rustacean (P) ☁");
    let colors = test_theme().colors;
    let (plan, _) = find_cell(&buffer, at, "(P)").expect("the plan badge");
    assert_eq!(buffer[(plan, at)].fg, colors.success);
    let cloud = (plan..buffer.area.width)
        .map(|x| &buffer[(x, at)])
        .find(|cell| cell.symbol() == "☁")
        .expect("the cloud after it");
    assert_eq!(cloud.fg, colors.muted, "the cloud keeps its own gray");
}
