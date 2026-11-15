//! Header, status bar and the chat input.

use super::{dot, muted, spinner};
use crate::app::{App, InputMode, Message, SideTab, INPUT_ID};
use crate::model::{Level, Phase};
use crate::{draw, slash, theme};
use iced::keyboard::{key::Named, Key};
use iced::widget::text::Wrapping;
use iced::widget::{
    button, canvas, column, container, row, rule, space, text, text_editor, tooltip, Column,
};
use iced::{Alignment, Element, Fill, Length};

/// Slash commands listed above the input at most.
const SLASH_MENU_ROWS: usize = 8;

pub fn header(app: &App) -> Element<'_, Message> {
    let aura = canvas(draw::Aura {
        t: app.clock(),
        busy: app.busy(),
    })
    .width(30)
    .height(30);

    let agent = app.model.active_agent();
    let subtitle = match agent {
        Some(agent) => match agent.project.as_deref().filter(|p| !p.is_empty()) {
            Some(project) => format!("{project} · {}", agent.name),
            None => agent.name.clone(),
        },
        None => "starting…".to_string(),
    };
    let title = column![
        text("AURA")
            .font(theme::ui_semibold())
            .size(15)
            .color(theme::WHITE),
        text(subtitle)
            .size(11)
            .color(theme::MUTED)
            .wrapping(Wrapping::None),
    ]
    .spacing(1);

    let model = &app.model;
    let model_label = model
        .models
        .iter()
        .find(|m| m.id == model.current_model)
        .map_or_else(
            || {
                if model.current_model.is_empty() {
                    "Model".to_string()
                } else {
                    model.current_model.clone()
                }
            },
            |m| m.label.clone(),
        );
    let model_button = button(
        row![
            text(model_label).size(12),
            text("▾").size(10).color(theme::MUTED)
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([5, 10])
    .style(theme::accent(theme::BLUE))
    .on_press(Message::OpenModels);

    let user = muted(model.user.clone().unwrap_or_else(|| "guest".to_string()));
    let api = tooltip(
        dot(if model.api_active {
            theme::CYAN
        } else {
            theme::FAINT
        }),
        container(
            text(
                model
                    .api_url
                    .clone()
                    .unwrap_or_else(|| "API server not running".to_string()),
            )
            .size(11)
            .font(theme::mono()),
        )
        .padding([4, 8])
        .style(theme::card),
        tooltip::Position::Bottom,
    );

    let tabs = row(SideTab::ALL.map(|tab| {
        let count = match tab {
            SideTab::Agents => model.agents.len(),
            SideTab::Records => model.records.len(),
            SideTab::Processes => model
                .processes
                .iter()
                .filter(|p| p.status.is_live())
                .count(),
            SideTab::Files => 0,
        };
        let label = if count > 0 {
            format!("{} {count}", tab.label())
        } else {
            tab.label().to_string()
        };
        button(text(label).size(12))
            .padding([5, 10])
            .style(theme::tab(app.side == Some(tab)))
            .on_press(Message::Side(tab))
            .into()
    }))
    .spacing(2);

    column![
        container(
            row![
                aura,
                title,
                space::horizontal(),
                model_button,
                user,
                api,
                tabs
            ]
            .spacing(14)
            .align_y(Alignment::Center),
        )
        .padding([10, 16])
        .style(theme::bar),
        rule::horizontal(1).style(|_| rule::Style {
            color: theme::LINE,
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: true,
        }),
    ]
    .into()
}

pub fn status_bar(app: &App) -> Element<'_, Message> {
    let model = &app.model;
    let mut bar = row![].spacing(12).align_y(Alignment::Center);
    if app.busy() {
        bar = bar.push(spinner(app, 11.0, theme::BLUE));
    }
    bar = bar.push(text(&model.status).size(12).color(theme::TEXT));
    if let Some((toast, level, _)) = &model.toast {
        let color = match level {
            Level::Info => theme::TEXT,
            Level::Success => theme::CYAN,
            Level::Warning => theme::PURPLE,
            Level::Error => theme::RED,
        };
        bar = bar.push(
            container(text(toast).size(12))
                .padding([1, 8])
                .style(theme::badge(color)),
        );
    }
    bar = bar.push(space::horizontal());
    if app.editing.is_some() {
        bar = bar.push(super::small_button(
            "Done editing",
            theme::CYAN,
            Message::DoneEditing,
        ));
    }
    let (phase, color) = match model.phase {
        Phase::Idle => ("idle", theme::FAINT),
        Phase::Processing => ("working", theme::BLUE),
        Phase::AwaitingApproval => ("needs approval", theme::PURPLE),
    };
    bar = bar.push(
        container(text(phase).size(11))
            .padding([1, 6])
            .style(theme::badge(color)),
    );
    if model.totals.calls > 0 {
        let mut usage = format!("{} tok", model.totals.tokens);
        if model.totals.cost_usd > 0.0 {
            usage.push_str(&format!(" · ${:.3}", model.totals.cost_usd));
        }
        bar = bar.push(muted(usage));
    }
    bar = bar.push(muted("Ctrl+/ help"));

    container(bar)
        .padding([6, 16])
        .width(Fill)
        .style(theme::bar)
        .into()
}

pub fn input(app: &App) -> Element<'_, Message> {
    let typed = app.input.text();
    let matches = if app.mode == InputMode::Chat {
        slash::matching(typed.trim_end_matches('\n'))
    } else {
        Vec::new()
    };

    let mut stack = Column::new().spacing(6).max_width(980);
    if !matches.is_empty() {
        let rows = matches.iter().take(SLASH_MENU_ROWS).map(|command| {
            button(
                row![
                    text(format!("/{}", command.name))
                        .font(theme::mono_medium())
                        .size(13)
                        .color(theme::WHITE),
                    text(command.args)
                        .font(theme::mono())
                        .size(12)
                        .color(theme::FAINT),
                    space::horizontal(),
                    muted(command.about),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .width(Fill)
            .padding([5, 10])
            .style(theme::ghost)
            .on_press(Message::PickSlash(command.name))
            .into()
        });
        stack = stack.push(
            container(Column::with_children(rows).spacing(1))
                .padding(4)
                .style(theme::card),
        );
    }

    let cursor = app.input.cursor().position;
    let first_line = cursor.line == 0;
    let last_line = cursor.line + 1 >= app.input.line_count();
    let menu_open = !matches.is_empty();
    let shell = app.mode == InputMode::Shell;
    let editor = text_editor(&app.input)
        .id(INPUT_ID)
        .placeholder(if shell {
            "Shell command — runs in the agent's workspace"
        } else {
            "Message AURA — / for commands, ! for a shell command"
        })
        .on_action(Message::Input)
        .font(if shell { theme::mono() } else { theme::ui() })
        .size(theme::TEXT_SIZE)
        .padding([10, 4])
        .height(Length::Shrink)
        .style(theme::editor)
        .key_binding(move |press| {
            if !matches!(press.status, text_editor::Status::Focused { .. }) {
                return None;
            }
            let plain = press.modifiers.is_empty();
            match press.key.as_ref() {
                Key::Named(Named::Enter) if !press.modifiers.shift() => {
                    Some(text_editor::Binding::Custom(Message::Submit))
                }
                Key::Named(Named::ArrowUp) if plain && first_line => {
                    Some(text_editor::Binding::Custom(Message::HistoryPrev))
                }
                Key::Named(Named::ArrowDown) if plain && last_line => {
                    Some(text_editor::Binding::Custom(Message::HistoryNext))
                }
                Key::Named(Named::Tab) if menu_open => {
                    Some(text_editor::Binding::Custom(Message::CompleteSlash))
                }
                Key::Named(Named::Escape) => Some(text_editor::Binding::Custom(Message::Escape)),
                _ => text_editor::Binding::from_key_press(press),
            }
        });

    let modes = row![
        mode_button("Chat", InputMode::Chat, app.mode),
        mode_button("Shell", InputMode::Shell, app.mode),
    ]
    .spacing(2);
    let action = if app.busy() {
        button(text("Stop").size(12))
            .padding([6, 12])
            .style(theme::accent(theme::RED))
            .on_press(Message::Cancel)
    } else {
        button(text("Send").size(12))
            .padding([6, 12])
            .style(theme::accent(theme::BLUE))
            .on_press(Message::Submit)
    };
    let accent = if shell { theme::CYAN } else { theme::BLUE };
    stack = stack.push(
        container(
            row![modes, editor, action]
                .spacing(10)
                .align_y(Alignment::Center),
        )
        .padding([4, 10])
        .style(theme::input_shell(accent)),
    );

    container(stack)
        .padding(super::pad(8.0, 28.0, 14.0, 28.0))
        .center_x(Fill)
        .into()
}

fn mode_button(
    label: &'static str,
    mode: InputMode,
    current: InputMode,
) -> Element<'static, Message> {
    button(text(label).size(11))
        .padding([4, 8])
        .style(theme::tab(mode == current))
        .on_press(Message::SetMode(mode))
        .into()
}
