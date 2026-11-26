use super::*;

#[test]
#[allow(clippy::cast_precision_loss)]
fn right_clicking_a_project_opens_its_menu() {
    let mut stage = stage();
    stage.seed(false);
    stage.draw();
    let text = stage.shell.text();
    let row = text
        .lines()
        .position(|line| line.contains("AGENT01"))
        .expect("the project is listed");
    let line = text.lines().nth(row).unwrap_or_default();
    let col = line
        .find("AGENT01")
        .map_or(0, |at| line[..at].chars().count());
    let cell = stage.shell.metrics.cell;
    let origin = stage
        .shell
        .metrics
        .origin(stage.shell.grid_area(), stage.shell.cols);
    let bar = tab_bar::height(&stage.shell.metrics);
    stage.shell.pointer_moved(Point::new(
        origin.x + (col as f32 + 0.5) * cell.width,
        origin.y + (row as f32 + 0.5) * cell.height + bar,
    ));
    stage.shell.button(MouseButton::Right, true, stage.now);
    stage.shell.button(MouseButton::Right, false, stage.now);
    stage.draw();
    let text = stage.shell.text();
    assert!(text.contains("Add agent"), "{text}");
    assert!(text.contains("Remove project…"), "{text}");
}

#[test]
fn copy_log_and_c_put_the_log_on_the_clipboard_and_say_so() {
    use aura_ui::events::{LogRunView, LogView};
    let mut stage = stage();
    stage.seed(false);
    stage.draw();
    let log = PaneTab::Chat(ChatTab::Log);
    let tab = stage.shell.chrome.spot(log).expect("the tab is drawn");
    stage.drag(tab, tab);
    let json = "{\"whole\":true}".to_string();
    stage.shell.commands([UiCommand::ForAgent {
        agent_id: "a1".into(),
        command: Box::new(UiCommand::LogView(Box::new(LogView {
            agent_id: "a1".into(),
            path: "/runs/r/log.json".into(),
            json: json.clone(),
            runs: vec![LogRunView {
                run_id: "r".into(),
                title: "fix it".into(),
                ..LogRunView::default()
            }],
        }))),
    }]);
    stage.draw();
    let _ = stage.shell.take_effects();
    let button = stage
        .shell
        .tabs
        .front()
        .tui
        .run_log()
        .copy_button_area()
        .expect("the button is drawn");
    let origin = stage
        .shell
        .metrics
        .origin(stage.shell.grid_area(), stage.shell.cols);
    let cell = stage.shell.metrics.cell;
    let on_button = Point::new(
        origin.x + (f32::from(button.x) + 1.5) * cell.width,
        origin.y + (f32::from(button.y) + 0.5) * cell.height,
    );
    stage.drag(on_button, on_button);
    assert_eq!(stage.shell.take_effects(), [Effect::Copy(json.clone())]);
    stage.shell.copied(json.len(), Ok(()));
    stage.draw();
    let text = stage.shell.text();
    assert!(text.contains("Copied the log"), "{text}");
    assert!(text.contains("✓ Copied"), "{text}");

    stage.type_text("c");
    stage.draw();
    assert_eq!(stage.shell.take_effects(), [Effect::Copy(json)]);
}

#[test]
fn dragging_over_text_selects_it_for_the_clipboard() {
    let mut stage = stage();
    stage.seed(false);
    stage.shell.commands([
        UiCommand::StartStreaming,
        UiCommand::AppendText("a reply from the agent".into()),
        UiCommand::FinishStreaming,
        UiCommand::Complete,
    ]);
    stage.draw();
    // The row of the reply, from its first cell to well past its end.
    let text = stage.shell.text();
    let row = text
        .lines()
        .position(|line| line.contains("a reply from"))
        .expect("the reply is drawn");
    let line = text.lines().nth(row).unwrap_or_default();
    let col = line
        .find("a reply")
        .map_or(0, |at| line[..at].chars().count());
    let cell = stage.shell.metrics.cell;
    let origin = stage
        .shell
        .metrics
        .origin(stage.shell.grid_area(), stage.shell.cols);
    #[allow(clippy::cast_precision_loss)]
    let at = |col: usize| {
        Point::new(
            origin.x + (col as f32 + 0.5) * cell.width,
            origin.y + (row as f32 + 0.5) * cell.height,
        )
    };
    stage.drag(at(col), at(col + 40));
    let copied = stage.shell.take_effects();
    assert!(
        matches!(&copied[..], [Effect::Copy(text)] if text.contains("a reply from the agent")),
        "{copied:?}"
    );
}
