use super::channels::{channel_info_app, info_row};
use super::*;

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

#[test]
fn test_channel_info_top_is_a_header_with_its_details_indented() {
    use ratatui::style::Modifier;
    let mut app = channel_info_app(None);
    let rows = render_rows(&mut app, 140);
    let header = info_row(&rows, "CHANNEL ");
    let column = rows[header].find("CHANNEL").unwrap();
    let at = |row: usize| {
        rows[row][column..]
            .split('│')
            .next()
            .unwrap_or("")
            .to_string()
    };
    assert!(
        at(header + 1).starts_with("  #zode"),
        "{:?}",
        at(header + 1)
    );
    assert!(
        at(header + 2).starts_with("  Zode release"),
        "{:?}",
        at(header + 2)
    );
    assert!(
        at(header + 3).starts_with("  Ship it"),
        "{:?}",
        at(header + 3)
    );
    assert!(
        at(header + 4).trim().is_empty(),
        "a blank row, then COORDINATOR"
    );
    assert!(at(header + 5).starts_with("COORDINATOR"));

    // Nothing on INFO is bold, the title included.
    let buffer = render_buffer(&mut app, 140);
    let x = u16::try_from(rows[header][..column].chars().count()).unwrap();
    for y in header..header + 12 {
        for x in x..x + 30 {
            let cell = &buffer[(x, u16::try_from(y).unwrap())];
            assert!(
                !cell.modifier.contains(Modifier::BOLD),
                "bold at {x},{y}: {:?}",
                rows[y]
            );
        }
    }
}
