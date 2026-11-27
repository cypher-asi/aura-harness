use super::*;

/// An app showing the CHANNELS tab with `#zode`, whose coordinator spawned
/// a worker.
pub(super) fn channels_app() -> crate::App {
    use crate::app::{AgentsTab, PaneTab};
    use crate::events::{AgentSummary, ChannelSummary, UiCommand};
    let member = |id: &str, name: &str, depth: usize| AgentSummary {
        id: id.to_string(),
        name: name.to_string(),
        depth,
        ..AgentSummary::default()
    };
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetChannels(vec![ChannelSummary {
        id: "c1".to_string(),
        slug: "zode".to_string(),
        title: "Zode release".to_string(),
        description: "Ship it".to_string(),
        coordinator_id: "co".to_string(),
        members: vec![member("co", "Coordinator", 0), member("wk", "Worker", 1)],
        ..ChannelSummary::default()
    }]));
    app.select_pane_tab(PaneTab::Agents(AgentsTab::Channels));
    app
}

#[test]
fn test_channels_tab_lists_channels_over_new_channel() {
    let mut app = channels_app();
    let rows = render_rows(&mut app, 120);
    let row_with = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} in:\n{}", rows.join("\n")))
    };
    // The channel opened, so the chat's bar names it too, with its title.
    let bar = row_with("Zode release");
    let channel = (0..rows.len())
        .find(|&row| row != bar && rows[row].contains("#  zode"))
        .unwrap_or_else(|| panic!("no #zode row in:\n{}", rows.join("\n")));
    let new_channel = row_with("New channel");
    assert!(rows[new_channel].contains('+'), "{}", rows[new_channel]);
    assert!(channel < new_channel);
    // The `#` is the row's icon, over the `+` of "+ New channel", and the
    // slug its name, as a project's folder and name are.
    let column = |row: usize, text: &str| {
        rows[row]
            .find(text)
            .map(|at| rows[row][..at].chars().count())
    };
    assert_eq!(
        column(channel, "#"),
        column(new_channel, "+"),
        "{}",
        rows[channel]
    );
    assert_eq!(
        column(channel, "zode"),
        column(new_channel, "New channel"),
        "{}",
        rows[channel]
    );
    // The row is the slug and nothing else: no description.
    assert!(!rows[channel].contains("Ship it"), "{}", rows[channel]);
    // "+ New channel" is pinned to the panel's last row, above its border.
    assert!(
        rows[new_channel + 1].contains('└'),
        "{}",
        rows[new_channel + 1]
    );
}

#[test]
fn test_channels_tab_says_when_there_are_none() {
    use crate::app::{AgentsTab, PaneTab};
    let mut app = crate::App::new();
    app.select_pane_tab(PaneTab::Agents(AgentsTab::Channels));
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains("No channels yet."), "{screen}");
    assert!(screen.contains("New channel"), "{screen}");
}

#[test]
fn test_an_open_channel_keeps_the_list_and_lists_its_members_on_the_right() {
    use crate::events::{ChannelSummary, UiCommand};
    let mut app = channels_app();
    let mut channels = app.channels().to_vec();
    channels.push(ChannelSummary {
        id: "c2".to_string(),
        slug: "docs".to_string(),
        ..ChannelSummary::default()
    });
    app.process_command(UiCommand::SetChannels(channels));
    app.open_channel_by_id("c1");
    let rows = render_rows(&mut app, 140);
    let screen = rows.join("\n");
    let at = |needle: &str| {
        rows.iter()
            .enumerate()
            .find_map(|(y, row)| row.find(needle).map(|x| (y, row[..x].chars().count())))
            .unwrap_or_else(|| panic!("no {needle:?} in:\n{screen}"))
    };
    // The left panel is still the list.
    at("#  docs");
    at("New channel");
    assert!(!screen.contains("‹ #zode"), "{screen}");
    // The members are in the right column, two rows each: the worker, a
    // subagent of no permanent agent, under its parent, and each one's
    // status in gray under its name.
    let coordinator = at("Coordinator");
    let worker = at("subagent");
    assert_eq!(worker.0, coordinator.0 + 2);
    assert!(worker.1 > coordinator.1, "the worker is indented");
    assert!(coordinator.1 > 100, "in the right column: {screen}");
    for (y, x) in [coordinator, worker] {
        let status = &rows[y + 1];
        let at = status.find("Idle").map(|i| status[..i].chars().count());
        assert_eq!(at, Some(x), "the status under the name: {status:?}");
    }
    // Only the channel's own tabs, and none of the agent panels.
    assert!(screen.contains("MEMBERS"), "{screen}");
    assert!(screen.contains("INFO"), "{screen}");
    for agent_feature in [
        "SANDBOX",
        "PROCESSES",
        "TRACE",
        "KERNEL",
        "POLICIES",
        "INBOX",
    ] {
        assert!(
            !screen.contains(agent_feature),
            "{agent_feature}:\n{screen}"
        );
    }
    for chat_tab in ["TERMINAL", "GRAPH"] {
        assert!(!screen.contains(chat_tab), "{chat_tab}:\n{screen}");
    }
}

/// `channels_app` with `#zode` open on its INFO tab, `#zode` working in
/// `project` when given.
pub(super) fn channel_info_app(project: Option<crate::events::ChannelProjectView>) -> crate::App {
    use crate::app::{PaneTab, SidePanelTab};
    use crate::events::UiCommand;
    let mut app = channels_app();
    let mut channels = app.channels().to_vec();
    channels[0].project = project;
    app.process_command(UiCommand::SetChannels(channels));
    app.open_channel_by_id("c1");
    app.select_pane_tab(PaneTab::Side(SidePanelTab::ChannelInfo));
    assert_eq!(app.side_tab(), SidePanelTab::ChannelInfo);
    app
}

/// The first of `rows` containing `needle` and not `PROJECTS`, which
/// the left panel may show.
pub(super) fn info_row(rows: &[String], needle: &str) -> usize {
    rows.iter()
        .position(|row| row.contains(needle) && !row.contains("PROJECTS"))
        .unwrap_or_else(|| panic!("no {needle:?} in:\n{}", rows.join("\n")))
}

#[test]
fn test_channel_info_tab_shows_what_the_channel_is() {
    use crate::events::ChannelProjectView;
    let mut app = channel_info_app(Some(ChannelProjectView {
        id: "p1".to_string(),
        name: "zodeproj".to_string(),
        folder: "/src/zodeproj".to_string(),
    }));
    let rows = render_rows(&mut app, 140);
    let description = info_row(&rows, "Ship it");
    let coordinator = info_row(&rows, "COORDINATOR");
    let members = info_row(&rows, "2 agents");
    let heading = info_row(&rows, "PROJECT");
    let project = info_row(&rows, "zodeproj");
    assert!(rows[project].contains("/src/zodeproj"), "{}", rows[project]);
    let placement = info_row(&rows, "AGENTS RUN");
    let local = info_row(&rows, "Locally");
    let order = [
        description,
        coordinator,
        members,
        heading,
        project,
        placement,
        local,
    ];
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
    let screen = rows.join("\n");
    assert!(!screen.contains("LINKED PROJECTS"), "{screen}");
    assert!(!screen.contains("No project"), "{screen}");
}

#[test]
fn test_channel_info_says_a_cloud_channel_needs_a_project_to_run_agents_there() {
    use crate::events::{ChannelProjectView, UiCommand};
    let zodeproj = ChannelProjectView {
        id: "p1".to_string(),
        name: "zodeproj".to_string(),
        folder: "/src/zodeproj".to_string(),
    };
    let missing = ChannelProjectView {
        folder: String::new(),
        ..zodeproj.clone()
    };
    for (project, falls_back) in [(Some(zodeproj), false), (None, true), (Some(missing), true)] {
        let mut app = channel_info_app(project.clone());
        let mut channels = app.channels().to_vec();
        channels[0].placement = crate::host::PlacementView::Cloud;
        app.process_command(UiCommand::SetChannels(channels));
        let rows = render_rows(&mut app, 140);
        let cloud = info_row(&rows, "In the cloud");
        assert!(info_row(&rows, "AGENTS RUN") < cloud, "{project:?}");
        let note = rows.iter().position(|row| row.contains("run locally"));
        assert_eq!(
            note.is_some_and(|note| note > cloud),
            falls_back,
            "{project:?}"
        );
    }
}

#[test]
fn test_channel_info_without_a_project_says_where_agents_work() {
    let mut app = channel_info_app(None);
    let rows = render_rows(&mut app, 140);
    let heading = info_row(&rows, "PROJECT");
    let none = info_row(&rows, "No project —");
    assert!(heading < none, "{heading} {none}");
    let info: String = rows[none..]
        .iter()
        .filter_map(|row| row.split('│').nth(5))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        info.contains("No project — agents work in the channel's folder"),
        "{info:?}"
    );
    assert!(!info.contains("……"), "wrapped, not cut: {info:?}");
    assert!(!info.contains("Right-click"), "no keys: {info:?}");
}

#[test]
fn test_channel_info_shows_a_missing_project_by_name_without_a_folder() {
    use crate::events::ChannelProjectView;
    let mut app = channel_info_app(Some(ChannelProjectView {
        id: "gone".to_string(),
        name: "(missing project)".to_string(),
        folder: String::new(),
    }));
    let rows = render_rows(&mut app, 140);
    let heading = info_row(&rows, "PROJECT");
    let project = info_row(&rows, "(missing project)");
    assert!(heading < project, "{heading} {project}");
    let cell = rows[project]
        .split('│')
        .nth(5)
        .map(str::trim)
        .unwrap_or_default();
    assert_eq!(cell, "(missing project)", "{}", rows[project]);
    assert!(!rows.join("\n").contains("No project"));
}

/// `channels_app` listing projects `alpha` and `beta`.
fn channels_app_with_projects() -> crate::App {
    use crate::events::{ProjectSummary, UiCommand};
    let project = |id: &str, name: &str| ProjectSummary {
        id: id.to_string(),
        name: name.to_string(),
        folder: format!("/src/{name}"),
        ..ProjectSummary::default()
    };
    let mut app = channels_app();
    app.process_command(UiCommand::SetProjects(vec![
        project("p1", "alpha"),
        project("p2", "beta"),
    ]));
    app
}

#[test]
fn test_the_new_channel_form_draws_the_project_chooser() {
    let mut app = channels_app_with_projects();
    app.start_new_channel();
    let rows = render_rows(&mut app, 140);
    let screen = rows.join("\n");
    assert!(screen.contains("┌ NEW CHANNEL ─"), "{screen}");
    let label = info_row(&rows, "Project");
    let chooser = info_row(&rows, "‹ None ›");
    assert!(label < chooser, "{label} {chooser}");
    let placement = info_row(&rows, "‹ Locally ›");
    assert!(chooser < placement, "{chooser} {placement}");
    assert!(
        rows[placement + 1].contains("New members run on this computer"),
        "{}",
        rows[placement + 1]
    );
    for hint in ["agents work in", "choose project", "Ctrl+Enter", "Esc"] {
        assert!(!screen.contains(hint), "no hints, {hint}:\n{screen}");
    }
}

#[test]
fn test_the_form_shows_a_chosen_project_and_its_folder() {
    use crossterm::event::{KeyCode, KeyEvent};
    let mut app = channels_app_with_projects();
    app.start_new_channel();
    app.handle_key(KeyEvent::from(KeyCode::BackTab));
    app.handle_key(KeyEvent::from(KeyCode::BackTab));
    app.handle_key(KeyEvent::from(KeyCode::Right));
    let rows = render_rows(&mut app, 140);
    let chooser = info_row(&rows, "‹ alpha ›");
    assert!(
        rows[chooser + 1].contains("/src/alpha"),
        "{}",
        rows[chooser + 1]
    );
}

#[test]
fn test_the_edit_form_is_titled_by_the_slug_and_shows_the_channels_project() {
    use crate::events::ChannelProjectView;
    let mut app = channel_info_app(Some(ChannelProjectView {
        id: "p9".to_string(),
        name: "zodeproj".to_string(),
        folder: "/src/zodeproj".to_string(),
    }));
    app.start_edit_channel("c1");
    let rows = render_rows(&mut app, 140);
    let screen = rows.join("\n");
    assert!(screen.contains("┌ EDIT #zode ─"), "{screen}");
    assert!(!screen.contains("NEW CHANNEL"), "{screen}");
    assert!(screen.contains("‹ zodeproj ›"), "{screen}");
    assert!(
        !screen.contains(crate::app::channel_form::SLUG_HINT),
        "{screen}"
    );
}

#[test]
fn test_channels_with_nothing_open_say_so_and_hide_the_right_column() {
    use crate::app::{AgentsTab, PaneTab};
    let mut app = crate::App::new();
    app.select_pane_tab(PaneTab::Agents(AgentsTab::Channels));
    let screen = render_rows(&mut app, 140).join("\n");
    assert!(screen.contains("Start one with + New channel"), "{screen}");
    for agent_feature in ["SANDBOX", "KERNEL", "TERMINAL", "GRAPH"] {
        assert!(
            !screen.contains(agent_feature),
            "{agent_feature}:\n{screen}"
        );
    }
}

#[test]
fn test_agents_app_without_an_instance_says_so_and_hides_the_right_column() {
    use crate::app::{AgentsTab, PaneTab};
    use crate::events::{AgentDefinitionSummary, UiCommand};
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgentDefinitions(vec![
        AgentDefinitionSummary {
            name: "explorer".to_string(),
            description: "Reads code".to_string(),
            abbreviation: None,
        },
    ]));
    app.select_pane_tab(PaneTab::Agents(AgentsTab::Roster));
    let screen = render_rows(&mut app, 140).join("\n");
    assert!(screen.contains("No explorer agent has run yet"), "{screen}");
    for agent_feature in ["SANDBOX", "KERNEL", "GRAPH"] {
        assert!(
            !screen.contains(agent_feature),
            "{agent_feature}:\n{screen}"
        );
    }
}

/// [`channels_app`] with the chat on `active` and `#zode` open.
fn channel_chat_app(active: &str) -> crate::App {
    use crate::events::UiCommand;
    let mut app = channels_app();
    app.process_command(UiCommand::SetActiveAgent(active.to_string()));
    app.open_channel_by_id("c1");
    app
}

#[test]
fn channel_posts_render_between_history_messages_by_time() {
    use crate::components::MessageRole;
    use crate::events::{ChannelPostAuthor, ChannelPostView, MessageData, UiCommand};

    let mut app = channel_chat_app("co");
    let at = |role, text: &str, ms| MessageData {
        timestamp_ms: Some(ms),
        ..MessageData::new(role, text)
    };
    app.process_command(UiCommand::LoadConversation(vec![
        at(MessageRole::User, "kickoff question", 1000),
        at(MessageRole::Assistant, "coordinator answer", 3000),
    ]));
    app.process_command(UiCommand::SetChannelPosts {
        channel_id: "c1".to_string(),
        posts: vec![ChannelPostView {
            seq: 2,
            author: ChannelPostAuthor::Agent {
                id: "wk".to_string(),
                code: Some("RE36".to_string()),
                name: "Worker".to_string(),
            },
            text: "zebra crossing report".to_string(),
            reply_to: Some(1),
            at_ms: 2000,
            ..ChannelPostView::default()
        }],
    });

    let rows = render_rows(&mut app, 120);
    let screen = rows.join("\n");
    let row_with = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} in:\n{screen}"))
    };
    let first = row_with("kickoff question");
    let post = row_with("zebra crossing report");
    let second = row_with("coordinator answer");
    assert!(
        first < post && post < second,
        "{first} {post} {second}:\n{screen}"
    );
    // The label sits above the post's text, after the first message.
    let label = (first + 1..post)
        .rev()
        .find(|&y| rows[y].contains("RE36"))
        .unwrap_or_else(|| panic!("no RE36 label above the post:\n{screen}"));
    assert!(rows[label].contains('↩'), "{}", rows[label]);
}

#[test]
fn channel_chat_header_names_the_channel_and_has_no_tabs() {
    let mut app = channel_chat_app("co");
    let rows = render_rows(&mut app, 140);
    let screen = rows.join("\n");
    // The chat bar: `#zode` with its title at the bar's far end. The list's
    // row on the left shows only the slug.
    let bar = rows
        .iter()
        .find(|row| row.contains("#zode") && row.contains("Zode release"))
        .unwrap_or_else(|| panic!("no #zode in the chat bar:\n{screen}"));
    for tab in ["CHAT", "CHT", "TERMINAL", "TRM", "LOG"] {
        assert!(!bar.contains(tab), "{tab}: {bar}");
    }
    assert!(screen.contains("Message #zode"), "{screen}");

    // Back among the agents, the tabs come back.
    app.select_pane_tab(crate::app::PaneTab::Agents(crate::app::AgentsTab::Projects));
    let screen = render_rows(&mut app, 140).join("\n");
    assert!(
        screen.contains("TERMINAL") || screen.contains("TRM"),
        "{screen}"
    );
}

#[test]
fn a_leading_plan_command_is_drawn_in_the_accent() {
    let theme = test_theme();
    let accent = theme.colors.success;
    let spans = super::super::input::chat_spans("/plan add @src/a.rs", &theme);
    assert_eq!(spans[0].content, "/plan");
    assert_eq!(spans[0].style.fg, Some(accent));
    assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert!(spans[1..].iter().all(|s| s.style.fg != Some(accent)));
    assert!(spans
        .iter()
        .any(|s| s.content == "@src/a.rs" && s.style.add_modifier.contains(Modifier::UNDERLINED)));

    for plain in ["/planner", "/pla", "src/plan", "/build x /plan"] {
        let spans = super::super::input::chat_spans(plain, &theme);
        assert!(spans.iter().all(|s| s.style.fg != Some(accent)), "{plain}");
    }

    // Anywhere in the draft, each `/plan` word takes the accent.
    let spans = super::super::input::chat_spans("fix it /plan then /PLAN", &theme);
    let lit: Vec<&str> = spans
        .iter()
        .filter(|s| s.style.fg == Some(accent))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(lit, ["/plan", "/PLAN"]);

    // On screen, the typed `/plan` takes the accent.
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = crate::App::new();
    for c in "fix it /plan".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
    }
    let buffer = render_buffer(&mut app, 120);
    let row = row_of(&buffer, "fix it /plan");
    let (_, slash) = find_cell(&buffer, row, "/plan").unwrap();
    assert_eq!(slash.fg, Some(accent));
}
