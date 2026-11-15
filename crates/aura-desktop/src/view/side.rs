//! The side panel: agents, kernel records, processes and the sandbox.

use super::{dot, duration, fixed, indent, muted, section, small_button};
use crate::app::{App, Confirm, Message, ProjectPurpose, SideTab, SIDE_WIDTH};
use crate::draw::{self, Timeline};
use crate::theme;
use aura_ui_core::events::{ProcessStatus, RecordStatus};
use aura_ui_core::sandbox::{SandboxEntryKind, MAX_COUNTED_FILES};
use iced::widget::text::Wrapping;
use iced::widget::{button, canvas, column, container, row, scrollable, space, text, Column};
use iced::{Alignment, Element, Fill};

pub fn view(app: &App, tab: SideTab) -> Element<'_, Message> {
    let body = match tab {
        SideTab::Agents => agents(app),
        SideTab::Records => records(app),
        SideTab::Processes => processes(app),
        SideTab::Files => files(app),
    };
    container(scrollable(container(body).padding([14, 14])).height(Fill))
        .width(fixed(SIDE_WIDTH))
        .height(Fill)
        .style(theme::panel)
        .into()
}

fn agents(app: &App) -> Element<'_, Message> {
    let model = &app.model;
    let mut list = Column::new().spacing(2);
    for agent in &model.agents {
        let active = agent.id == model.active_agent;
        let mut line = row![
            indent(agent.depth, 14.0),
            dot(if agent.running {
                theme::BLUE
            } else if active {
                theme::CYAN
            } else {
                theme::FAINT
            }),
            column![
                text(
                    agent
                        .project
                        .as_deref()
                        .filter(|p| !p.is_empty())
                        .unwrap_or(&agent.name)
                )
                .size(13)
                .wrapping(Wrapping::None),
                text(&agent.zns_id)
                    .size(11)
                    .color(theme::FAINT)
                    .font(theme::mono()),
            ]
            .spacing(1),
            space::horizontal(),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        if let Some(definition) = &agent.definition {
            line = line.push(
                container(text(definition).size(10))
                    .padding([1, 6])
                    .style(theme::badge(theme::PURPLE)),
            );
        }
        line = line.push(
            column![
                text(&agent.last_active).size(11).color(theme::MUTED),
                text(format!("{} rec", agent.record_count))
                    .size(10)
                    .color(theme::FAINT),
            ]
            .align_x(Alignment::End),
        );
        list = list.push(
            button(line)
                .width(Fill)
                .padding([6, 8])
                .style(theme::list_row(active))
                .on_press(Message::SelectAgent(agent.id.clone())),
        );
    }
    if model.agents.is_empty() {
        list = list.push(muted("No agents yet"));
    }

    let mut roster = Column::new().spacing(8);
    for definition in &model.definitions {
        roster = roster.push(
            container(
                column![
                    text(&definition.name)
                        .size(13)
                        .font(theme::ui_medium())
                        .color(theme::WHITE),
                    text(&definition.description).size(12).color(theme::MUTED),
                    row![
                        small_button(
                            "Spawn",
                            theme::CYAN,
                            Message::Spawn(definition.name.clone())
                        ),
                        small_button(
                            "Add to project",
                            theme::BLUE,
                            Message::AddToProject(definition.name.clone()),
                        ),
                        space::horizontal(),
                        small_button(
                            "Edit",
                            theme::MUTED,
                            Message::EditDefinition(Some(definition.name.clone())),
                        ),
                        small_button(
                            "Delete",
                            theme::RED,
                            Message::Ask(Confirm::DeleteDefinition(definition.name.clone())),
                        ),
                    ]
                    .spacing(6),
                ]
                .spacing(6),
            )
            .padding(10)
            .width(Fill)
            .style(theme::card),
        );
    }
    if model.definitions.is_empty() {
        roster = roster.push(muted("No permanent agents defined"));
    }

    column![
        row![
            section("AGENTS"),
            space::horizontal(),
            small_button("+ Agent", theme::CYAN, Message::NewAgent),
            small_button(
                "+ Project",
                theme::BLUE,
                Message::OpenProject(ProjectPurpose::Create)
            ),
            small_button(
                "Rename",
                theme::MUTED,
                Message::OpenProject(ProjectPurpose::Rename)
            ),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        list,
        space().height(10),
        row![
            section("PERMANENT AGENTS"),
            space::horizontal(),
            small_button("+ New", theme::PURPLE, Message::EditDefinition(None)),
        ]
        .align_y(Alignment::Center),
        roster,
    ]
    .spacing(10)
    .into()
}

pub(crate) fn record_color(status: RecordStatus) -> iced::Color {
    match status {
        RecordStatus::Ok => theme::CYAN,
        RecordStatus::Error => theme::RED,
        RecordStatus::Pending => theme::PURPLE,
        RecordStatus::None => theme::FAINT,
    }
}

fn records(app: &App) -> Element<'_, Message> {
    let mut list = Column::new().spacing(2);
    for record in app.model.records.iter().rev() {
        let what = if record.what.is_empty() {
            &record.info
        } else {
            &record.what
        };
        let detail = if record.detail.is_empty() {
            &record.message
        } else {
            &record.detail
        };
        list = list.push(
            button(
                column![
                    row![
                        text(format!("#{}", record.seq))
                            .font(theme::mono_medium())
                            .size(12)
                            .color(theme::WHITE),
                        text(&record.tx_kind).size(12).color(theme::BLUE),
                        text(what)
                            .size(12)
                            .color(theme::TEXT)
                            .wrapping(Wrapping::None),
                        space::horizontal(),
                        dot(record_color(record.status)),
                        text(&record.timestamp)
                            .size(11)
                            .font(theme::mono())
                            .color(theme::FAINT),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    text(detail)
                        .size(11)
                        .color(theme::MUTED)
                        .wrapping(Wrapping::None),
                ]
                .spacing(2),
            )
            .width(Fill)
            .padding([6, 8])
            .style(theme::ghost)
            .on_press(Message::OpenRecord(record.seq)),
        );
    }
    if app.model.records.is_empty() {
        list = list.push(muted("Kernel records appear here as the agent works"));
    }
    column![section("RECORDS"), list].spacing(10).into()
}

fn processes(app: &App) -> Element<'_, Message> {
    let model = &app.model;
    let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or_default();
    let timeline = canvas(Timeline {
        processes: &model.processes,
        now_ms,
    })
    .width(Fill)
    .height(Timeline::height(model.processes.len()));

    let mut list = Column::new().spacing(2);
    for process in &model.processes {
        let color = draw::status_color(process.status);
        let ran = process
            .ended_ms
            .unwrap_or(now_ms)
            .saturating_sub(process.started_ms);
        let mut actions = row![].spacing(4);
        if !process.jump_agent_id.is_empty() {
            actions = actions.push(small_button(
                "Open",
                theme::BLUE,
                Message::SelectAgent(process.jump_agent_id.clone()),
            ));
        }
        if process.killable && process.status.is_live() {
            actions = actions.push(small_button(
                "Kill",
                theme::RED,
                Message::Ask(Confirm::Kill(process.id.clone())),
            ));
        }
        let mut info = vec![process.owner_name.clone(), duration(ran)];
        if let Some(code) = process
            .exit_code
            .filter(|_| process.status == ProcessStatus::Failed)
        {
            info.push(format!("exit {code}"));
        }
        if let Some(model) = &process.model {
            info.push(model.clone());
        }
        list = list.push(
            container(
                row![
                    indent(process.depth, 12.0),
                    dot(color),
                    column![
                        text(&process.label).size(13).wrapping(Wrapping::None),
                        text(info.join(" · "))
                            .size(11)
                            .color(theme::FAINT)
                            .wrapping(Wrapping::None),
                    ]
                    .spacing(1),
                    space::horizontal(),
                    container(text(process.status.label()).size(10))
                        .padding([1, 6])
                        .style(theme::badge(color)),
                    actions,
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding([6, 8]),
        );
    }
    if model.processes.is_empty() {
        list = list.push(muted(
            "Agent turns, background agents and commands show up here",
        ));
    }
    column![section("PROCESSES"), timeline, list]
        .spacing(10)
        .into()
}

fn files(app: &App) -> Element<'_, Message> {
    let sandbox = &app.model.sandbox;
    let root = sandbox
        .root()
        .map_or_else(|| "No sandbox yet".to_string(), |r| r.display().to_string());
    let mut list = Column::new();
    for (index, entry) in sandbox.rows().iter().enumerate() {
        let (icon, color) = match entry.kind {
            SandboxEntryKind::Dir if entry.expanded => ("▾", theme::BLUE),
            SandboxEntryKind::Dir => ("▸", theme::BLUE),
            SandboxEntryKind::File => ("·", theme::FAINT),
            SandboxEntryKind::Symlink => ("↪", theme::PURPLE),
            SandboxEntryKind::More(_) => ("…", theme::FAINT),
        };
        let name = match entry.kind {
            SandboxEntryKind::More(n) => format!("{n} more"),
            _ => entry.name.clone(),
        };
        list = list.push(
            button(
                row![
                    indent(entry.depth, 14.0),
                    text(icon).size(12).color(color).width(12),
                    text(name)
                        .size(12)
                        .font(theme::mono())
                        .color(if entry.kind == SandboxEntryKind::Dir {
                            theme::WHITE
                        } else {
                            theme::TEXT
                        })
                        .wrapping(Wrapping::None),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .width(Fill)
            .padding([3, 6])
            .style(theme::list_row(index == sandbox.selected()))
            .on_press(Message::SandboxRow(index)),
        );
    }
    column![
        row![
            section("FILES"),
            space::horizontal(),
            muted(match sandbox.file_count() {
                Some(n) if n >= MAX_COUNTED_FILES => format!("{n}+ files"),
                Some(n) => format!("{n} files"),
                None => String::new(),
            })
            .size(11),
            small_button("Refresh", theme::BLUE, Message::RefreshFiles),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        text(root).size(11).font(theme::mono()).color(theme::FAINT),
        list,
    ]
    .spacing(8)
    .into()
}
