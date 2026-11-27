use super::*;

/// An app with three models whose bench form is open on arm B set to
/// pick a model by difficulty, none picked yet.
fn bench_by_difficulty_app() -> crate::App {
    use crate::app::BY_DIFFICULTY;
    use crate::events::{ModelOptionView, UiCommand};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let model = |id: &str, label: &str, group: &str| ModelOptionView {
        id: id.to_string(),
        label: label.to_string(),
        local: false,
        installed: true,
        group: group.to_string(),
        provider: None,
        available: true,
        context_window: None,
    };
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetModels {
        options: vec![
            model("gate", "Gate", "Gate"),
            model(
                "claude/claude-sonnet-4-6",
                "Claude Sonnet 4.6",
                "Claude plan",
            ),
            model(
                "openrouter/deepseek/deepseek-v3.2",
                "DeepSeek V3.2",
                "OpenRouter",
            ),
        ],
        current: "gate".to_string(),
        route: None,
        user: None,
        plans: Vec::new(),
        thinking: None,
    });
    app.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT));
    while app.focus() != crate::app::PanelFocus::Swarm {
        app.handle_key(KeyEvent::from(KeyCode::Tab));
    }
    app.handle_key(KeyEvent::from(KeyCode::Char('n')));
    assert_eq!(app.bench().mode, Some(crate::app::BenchMode::Form));
    app.bench_mut().form.b = BY_DIFFICULTY.to_string();
    app.bench_mut().form.b_models = Default::default();
    app
}

fn click(app: &mut crate::App, column: u16, row: u16) {
    use crossterm::event::{MouseButton, MouseEventKind};

    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        mouse_at(app, kind, (column, row));
    }
}

#[test]
fn test_clicking_a_difficulty_row_then_a_model_picks_it_from_the_selector() {
    use crate::app::{AppState, PickerTarget};

    let mut app = bench_by_difficulty_app();
    let buffer = render_buffer(&mut app, 160);
    let row = row_of(&buffer, "  Medium");
    let (column, _) = find_cell(&buffer, row, "Medium").unwrap();
    click(&mut app, column, row);
    assert_eq!(app.state(), AppState::SelectingModel);
    assert_eq!(
        app.model_picker().target(),
        PickerTarget::BenchDifficulty {
            arm: 1,
            difficulty: 1
        }
    );

    let buffer = render_buffer(&mut app, 160);
    assert!(
        row_of(&buffer, "MODEL FOR MEDIUM WORK · ARM B") < row_of(&buffer, "DeepSeek V3.2"),
        "the selector is up"
    );
    let row = row_of(&buffer, "DeepSeek V3.2");
    let (column, _) = find_cell(&buffer, row, "DeepSeek V3.2").unwrap();
    click(&mut app, column, row);
    assert_eq!(app.state(), AppState::Idle);
    assert_eq!(
        app.bench().form.b_models[1],
        "openrouter/deepseek/deepseek-v3.2"
    );
    assert_eq!(app.current_model(), "gate", "the session keeps its model");

    // A click outside the selector closes it and changes nothing.
    let buffer = render_buffer(&mut app, 160);
    let row = row_of(&buffer, "  Hard");
    let (column, _) = find_cell(&buffer, row, "Hard").unwrap();
    click(&mut app, column, row);
    assert_eq!(app.state(), AppState::SelectingModel);
    render_buffer(&mut app, 160);
    click(&mut app, 0, 0);
    assert_eq!(app.state(), AppState::Idle);
    assert!(app.bench().form.b_models[2].is_empty());
}

/// `/model` on an app holding `count` local models (all ready) and one
/// Claude plan model that needs `/login claude`, drawn `height` rows tall.
fn model_selector_app(count: usize) -> crate::App {
    use crate::events::{ModelOptionView, UiCommand};

    let model = |id: String, label: String, group: &str, provider: Option<&str>, available| {
        ModelOptionView {
            id,
            label,
            local: provider.is_none(),
            installed: true,
            group: group.to_string(),
            provider: provider.map(str::to_string),
            available,
            context_window: None,
        }
    };
    let mut options: Vec<ModelOptionView> = (0..count)
        .map(|i| {
            model(
                format!("local/m{i:02}"),
                format!("Model {i:02}"),
                "Local",
                None,
                true,
            )
        })
        .collect();
    options.push(model(
        "claude/claude-sonnet-4-6".to_string(),
        "Claude Sonnet 4.6".to_string(),
        "Claude plan",
        Some("claude"),
        false,
    ));
    let mut app = crate::App::new();
    app.process_command(UiCommand::SetModels {
        options,
        current: "local/m00".to_string(),
        route: None,
        user: None,
        plans: Vec::new(),
        thinking: None,
    });
    type_text(&mut app, "/model");
    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::empty(),
    ));
    app
}

fn render_rows_tall(app: &mut crate::App, width: u16, height: u16) -> Vec<String> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, app, &test_theme()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

#[test]
fn test_model_selector_shows_tabs_and_counts() {
    let mut app = model_selector_app(3);
    let screen = render_rows_tall(&mut app, 100, 30).join("\n");
    assert!(screen.contains("SELECT MODEL"), "{screen}");
    assert!(screen.contains("Ready (3)"), "{screen}");
    assert!(screen.contains("Sign in (1)"), "{screen}");
    assert!(!screen.contains("Type to filter"), "no key hints: {screen}");
    assert!(screen.contains("● Model 00"), "{screen}");
    assert!(
        !screen.contains("Claude Sonnet 4.6"),
        "the signed-out model waits on the other tab: {screen}"
    );
}

#[test]
fn test_model_selector_scrolls_to_keep_the_cursor_in_view() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = model_selector_app(60);
    let rows = render_rows_tall(&mut app, 100, 20);
    let screen = rows.join("\n");
    assert!(screen.contains("Model 00"), "{screen}");
    assert!(!screen.contains("Model 59"), "{screen}");
    let thumb_beside = |rows: &[String], text: &str| {
        rows.iter()
            .find(|row| row.contains(text))
            .is_some_and(|row| row.trim_end().ends_with('┃'))
    };
    assert!(
        thumb_beside(&rows, "Local"),
        "the scrollbar's thumb is at the top: {screen}"
    );

    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::empty()));
    let rows = render_rows_tall(&mut app, 100, 20);
    let screen = rows.join("\n");
    assert!(screen.contains("›   Model 59"), "{screen}");
    assert!(!screen.contains("Model 00"), "{screen}");
    assert!(
        thumb_beside(&rows, "Model 59"),
        "the scrollbar's thumb is at the bottom: {screen}"
    );
    assert!(!screen.contains("more"), "no more-rows: {screen}");
}

#[test]
fn test_model_selector_sign_in_tab_shows_the_login_to_run() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = model_selector_app(3);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
    let screen = render_rows_tall(&mut app, 100, 30).join("\n");
    assert!(
        screen.contains("›   Claude Sonnet 4.6  /login claude"),
        "{screen}"
    );
    assert!(!screen.contains("Model 00"), "{screen}");
}

#[test]
fn test_model_selector_filters_as_you_type() {
    let mut app = model_selector_app(12);
    type_text(&mut app, "l 1");
    let screen = render_rows_tall(&mut app, 100, 30).join("\n");
    assert!(screen.contains("/ l 1█"), "{screen}");
    assert!(
        screen.contains("Model 10") && screen.contains("Model 11"),
        "{screen}"
    );
    assert!(!screen.contains("Model 02"), "{screen}");

    type_text(&mut app, "zz");
    let screen = render_rows_tall(&mut app, 100, 30).join("\n");
    assert!(screen.contains("No models match"), "{screen}");
}
