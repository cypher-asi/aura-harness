//! Modal dialogs. An approval request outranks a confirmation, which
//! outranks any other open dialog.

use super::{muted, section, small_button};
use crate::app::{App, Confirm, Message, Overlay, ProjectPurpose};
use crate::{slash, theme};
use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, row, scrollable, space, text, text_input, Column};
use iced::{Alignment, Element, Fill, Length};

pub fn view(app: &App) -> Option<Element<'_, Message>> {
    if let Some(approval) = &app.model.approval {
        return Some(modal(
            "Approve tool call?",
            column![
                text(&approval.tool)
                    .font(theme::mono_medium())
                    .size(14)
                    .color(theme::PURPLE),
                text(&approval.description).size(13),
                row![
                    space::horizontal(),
                    action("Deny  n", theme::RED, Message::Deny),
                    action("Approve  y", theme::CYAN, Message::Approve),
                ]
                .spacing(8),
            ]
            .spacing(12)
            .into(),
            480.0,
        ));
    }
    if let Some(confirm) = &app.confirm {
        let question = match confirm {
            Confirm::DeleteDefinition(name) => format!("Delete the permanent agent {name}?"),
            Confirm::Kill(id) => {
                let label = app
                    .model
                    .processes
                    .iter()
                    .find(|p| &p.id == id)
                    .map_or(id.as_str(), |p| p.label.as_str());
                format!("Stop {label}?")
            }
        };
        return Some(modal(
            "Are you sure?",
            column![
                text(question).size(13),
                row![
                    space::horizontal(),
                    action("Cancel", theme::MUTED, Message::ConfirmNo),
                    action("Yes", theme::RED, Message::ConfirmYes),
                ]
                .spacing(8),
            ]
            .spacing(12)
            .into(),
            420.0,
        ));
    }
    let overlay = app.overlay.as_ref()?;
    Some(match overlay {
        Overlay::Help => modal("Help", help(), 720.0),
        Overlay::Models => modal("Choose a model", models(app), 560.0),
        Overlay::Login { email, password } => modal(
            "Sign in to zOS",
            column![
                text_input("Email", email)
                    .on_input(Message::LoginEmail)
                    .on_submit(Message::LoginSubmit)
                    .padding(8)
                    .style(theme::field),
                text_input("Password", password)
                    .secure(true)
                    .on_input(Message::LoginPassword)
                    .on_submit(Message::LoginSubmit)
                    .padding(8)
                    .style(theme::field),
                buttons("Sign in", Message::LoginSubmit),
            ]
            .spacing(10)
            .into(),
            420.0,
        ),
        Overlay::LinkCode { code } => {
            let provider = app.model.link_prompt.as_deref().unwrap_or("the provider");
            modal(
                "Paste the sign-in code",
                column![
                    muted(format!(
                        "Finish signing in to {provider} in your browser, then paste the code it shows."
                    )),
                    text_input("Code", code)
                        .on_input(Message::LinkCodeChanged)
                        .on_submit(Message::LinkCodeSubmit)
                        .font(theme::mono())
                        .padding(8)
                        .style(theme::field),
                    buttons("Link", Message::LinkCodeSubmit),
                ]
                .spacing(10)
                .into(),
                460.0,
            )
        }
        Overlay::Record(seq) => modal(&format!("Record #{seq}"), record(app, *seq), 680.0),
        Overlay::File { path, text: body } => {
            let content: Element<'_, Message> = match body {
                Ok(body) => super::mono_line(body.as_str())
                    .wrapping(Wrapping::WordOrGlyph)
                    .into(),
                Err(e) => text(e).size(13).color(theme::RED).into(),
            };
            let mut header = row![
                muted(path.display().to_string()).font(theme::mono()),
                space::horizontal(),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            if let Ok(body) = body {
                header = header.push(small_button(
                    "Copy",
                    theme::BLUE,
                    Message::Copy(body.clone()),
                ));
            }
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            modal_owned(
                name,
                column![
                    header,
                    container(
                        scrollable(container(content).padding(10)).height(Length::Fixed(520.0))
                    )
                    .style(theme::code)
                    .width(Fill),
                ]
                .spacing(10)
                .into(),
                820.0,
            )
        }
        Overlay::Project {
            purpose,
            name,
            folder,
        } => {
            let (title, submit) = match purpose {
                ProjectPurpose::Create => ("New project", "Create"),
                ProjectPurpose::Rename => ("Rename project", "Rename"),
            };
            let mut form = column![text_input("Project name", name)
                .on_input(Message::ProjectName)
                .on_submit(Message::ProjectSubmit)
                .padding(8)
                .style(theme::field)]
            .spacing(10);
            if *purpose == ProjectPurpose::Create {
                form = form.push(
                    text_input("Folder", folder)
                        .on_input(Message::ProjectFolder)
                        .on_submit(Message::ProjectSubmit)
                        .font(theme::mono())
                        .padding(8)
                        .style(theme::field),
                );
            }
            modal(
                title,
                form.push(buttons(submit, Message::ProjectSubmit)).into(),
                460.0,
            )
        }
    })
}

fn modal<'a>(title: &str, body: Element<'a, Message>, width: f32) -> Element<'a, Message> {
    modal_owned(title.to_string(), body, width)
}

fn modal_owned<'a>(title: String, body: Element<'a, Message>, width: f32) -> Element<'a, Message> {
    container(
        column![
            row![
                text(title)
                    .size(16)
                    .font(theme::ui_semibold())
                    .color(theme::WHITE),
                space::horizontal(),
                button(muted("Esc"))
                    .padding([2, 6])
                    .style(theme::ghost)
                    .on_press(Message::CloseOverlay),
            ]
            .align_y(Alignment::Center),
            body,
        ]
        .spacing(14),
    )
    .padding(20)
    .max_width(width)
    .style(theme::modal)
    .into()
}

fn action(label: &str, color: iced::Color, message: Message) -> Element<'_, Message> {
    button(text(label).size(13))
        .padding([6, 14])
        .style(theme::accent(color))
        .on_press(message)
        .into()
}

fn buttons(submit: &str, message: Message) -> Element<'_, Message> {
    row![
        space::horizontal(),
        action("Cancel", theme::MUTED, Message::CloseOverlay),
        action(submit, theme::BLUE, message),
    ]
    .spacing(8)
    .into()
}

fn help<'a>() -> Element<'a, Message> {
    let keys = [
        ("Enter", "send"),
        ("Shift+Enter", "new line"),
        ("Up / Down", "input history"),
        ("Tab", "complete a slash command"),
        ("Esc", "close a dialog, or stop the agent"),
        ("Ctrl+K", "choose a model"),
        ("Ctrl+B", "toggle the side panel"),
        ("Ctrl+1…4", "agents, records, processes, files"),
        ("Ctrl+O", "expand or collapse all steps"),
        ("Ctrl+J", "switch between chat and shell"),
        ("Ctrl+C", "stop the agent while it works"),
        ("y / n", "approve or deny a tool call"),
    ];
    let key_rows = keys.map(|(key, what)| {
        row![
            text(key)
                .font(theme::mono_medium())
                .size(12)
                .color(theme::BLUE)
                .width(120),
            text(what).size(13),
        ]
        .into()
    });
    let command_rows = slash::COMMANDS.iter().map(|c| {
        row![
            text(format!("/{} {}", c.name, c.args))
                .font(theme::mono())
                .size(12)
                .color(theme::CYAN)
                .width(260),
            text(c.about).size(13).color(theme::MUTED),
        ]
        .into()
    });
    scrollable(
        column![
            section("KEYS"),
            Column::with_children(key_rows).spacing(4),
            section("COMMANDS"),
            Column::with_children(command_rows).spacing(4),
        ]
        .spacing(10)
        .padding(super::pad(0.0, 16.0, 0.0, 0.0)),
    )
    .height(Length::Fixed(520.0))
    .into()
}

fn models(app: &App) -> Element<'_, Message> {
    let mut list = Column::new().spacing(2);
    let mut group = None;
    for option in &app.model.models {
        if group != Some(option.group.as_str()) {
            group = Some(option.group.as_str());
            list = list
                .push(container(section(&option.group)).padding(super::pad(8.0, 4.0, 2.0, 4.0)));
        }
        let current = option.id == app.model.current_model;
        let note = if !option.available {
            option.provider.as_deref().map_or_else(
                || "sign-in required".to_string(),
                |p| format!("sign in to {p}"),
            )
        } else if option.local && !option.installed {
            "not installed".to_string()
        } else {
            String::new()
        };
        list = list.push(
            button(
                row![
                    text(if current { "●" } else { " " })
                        .size(10)
                        .color(theme::CYAN)
                        .width(12),
                    text(&option.label).size(13),
                    text(&option.id)
                        .size(11)
                        .font(theme::mono())
                        .color(theme::FAINT),
                    space::horizontal(),
                    text(note).size(11).color(theme::PURPLE),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .width(Fill)
            .padding([6, 8])
            .style(theme::list_row(current))
            .on_press(Message::ChooseModel(option.id.clone())),
        );
    }
    container(scrollable(list)).max_height(440).into()
}

fn record(app: &App, seq: u64) -> Element<'_, Message> {
    let Some(record) = app.model.records.iter().find(|r| r.seq == seq) else {
        return muted("This record is no longer kept.").into();
    };
    let field = |name: &'static str, value: String| -> Element<'_, Message> {
        row![
            text(name).size(12).color(theme::FAINT).width(110),
            text(value)
                .size(12)
                .font(theme::mono())
                .wrapping(Wrapping::WordOrGlyph),
        ]
        .into()
    };
    let mut fields = vec![
        field("kind", record.tx_kind.clone()),
        field("time", record.timestamp.clone()),
        field("sender", record.sender.clone()),
        field("status", format!("{:?}", record.status).to_lowercase()),
        field("effects", record.effect_status.clone()),
        field("actions", record.action_count.to_string()),
        field("hash", record.full_hash.clone()),
    ];
    if !record.tx_id.is_empty() {
        fields.push(field("tx", record.tx_id.clone()));
    }
    if !record.agent_id.is_empty() {
        fields.push(field("agent", record.agent_id.clone()));
    }
    if !record.info.is_empty() {
        fields.push(field("info", record.info.clone()));
    }
    if let Some(policy) = &record.policy {
        fields.push(field("verdict", policy.verdict.clone()));
        fields.push(field("decided by", policy.decided_by.clone()));
        if !policy.reason.is_empty() {
            fields.push(field("reason", policy.reason.clone()));
        }
        if !policy.trace.is_empty() {
            fields.push(field("trace", policy.trace.join("\n")));
        }
        fields.push(field("mode", policy.mode.clone()));
    }
    let mut body = column![Column::with_children(fields).spacing(4)].spacing(12);
    if !record.message.is_empty() {
        body = body.push(section("MESSAGE"));
        body = body.push(
            container(
                text(&record.message)
                    .size(12)
                    .font(theme::mono())
                    .wrapping(Wrapping::WordOrGlyph),
            )
            .padding(10)
            .width(Fill)
            .style(theme::code),
        );
    }
    if !record.error_details.is_empty() {
        body = body.push(section("ERROR"));
        body = body.push(text(&record.error_details).size(12).color(theme::RED));
    }
    container(scrollable(body)).max_height(480).into()
}
