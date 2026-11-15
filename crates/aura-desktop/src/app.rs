//! The iced application: messages, update logic and subscriptions.

use crate::model::{Effect, Level, Model, Phase};
use crate::perf::Perf;
use crate::{slash, theme, view};
use aura_ui_core::events::{UiCommand, UiEvent};
use aura_ui_core::history::InputHistory;
use aura_ui_core::sandbox::SandboxEntryKind;
use futures::Stream;
use iced::keyboard::{self, key::Named, Key};
use iced::widget::{operation, text_editor};
use iced::{event, window, Element, Size, Subscription, Task, Theme};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub const TRANSCRIPT_ID: &str = "transcript";
pub const INPUT_ID: &str = "input";
/// Entries shown at first and added by each "Show earlier".
pub const PAGE: usize = 200;
/// Width of the side panel.
pub const SIDE_WIDTH: f32 = 380.0;
/// Largest file the Files panel opens.
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Most commands applied in one update.
const MAX_BATCH: usize = 1024;

/// The channel pair connecting the UI to an agent session.
pub struct Handles {
    pub events: mpsc::Sender<UiEvent>,
    pub commands: mpsc::Receiver<UiCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Chat,
    Shell,
}

impl InputMode {
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Chat => Self::Shell,
            Self::Shell => Self::Chat,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideTab {
    Agents,
    Records,
    Processes,
    Files,
}

impl SideTab {
    pub const ALL: [Self; 4] = [Self::Agents, Self::Records, Self::Processes, Self::Files];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Agents => "Agents",
            Self::Records => "Records",
            Self::Processes => "Processes",
            Self::Files => "Files",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectPurpose {
    Create,
    Rename,
}

pub enum Overlay {
    Help,
    Models,
    Login {
        email: String,
        password: String,
    },
    LinkCode {
        code: String,
    },
    Record(u64),
    File {
        path: PathBuf,
        text: Result<String, String>,
    },
    Project {
        purpose: ProjectPurpose,
        name: String,
        folder: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    DeleteDefinition(String),
    Kill(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    Commands(Vec<UiCommand>),
    SessionEnded,
    Input(text_editor::Action),
    Submit,
    HistoryPrev,
    HistoryNext,
    CompleteSlash,
    PickSlash(&'static str),
    Escape,
    Cancel,
    SetMode(InputMode),
    Side(SideTab),
    ToggleEntry(u64),
    ToggleExpandAll,
    ShowEarlier,
    Copy(String),
    LinkClicked(String),
    Approve,
    Deny,
    OpenModels,
    ChooseModel(String),
    OpenHelp,
    CloseOverlay,
    LoginEmail(String),
    LoginPassword(String),
    LoginSubmit,
    LinkCodeChanged(String),
    LinkCodeSubmit,
    OpenProject(ProjectPurpose),
    ProjectName(String),
    ProjectFolder(String),
    ProjectSubmit,
    SelectAgent(String),
    NewAgent,
    Spawn(String),
    AddToProject(String),
    EditDefinition(Option<String>),
    Ask(Confirm),
    ConfirmYes,
    ConfirmNo,
    OpenRecord(u64),
    SandboxRow(usize),
    RefreshFiles,
    FileLoaded(PathBuf, Result<String, String>),
    EditorClosed(PathBuf),
    DoneEditing,
    Key(keyboard::Event),
    Tick(Instant),
    Toasts(Instant),
    Resized(Size),
}

pub struct App {
    pub model: Model,
    events: mpsc::Sender<UiEvent>,
    pub input: text_editor::Content,
    history: InputHistory,
    pub mode: InputMode,
    pub side: Option<SideTab>,
    pub overlay: Option<Overlay>,
    pub confirm: Option<Confirm>,
    /// Entries shown from the end of the conversation
    pub visible: usize,
    pub started: Instant,
    pub now: Instant,
    pub width: f32,
    /// File opened in an external editor that returned at once
    pub editing: Option<PathBuf>,
    pub session_ended: bool,
    pub perf: Perf,
}

/// Run the desktop app on `handles` until the window closes.
///
/// # Errors
///
/// When the window or GPU context can't be created.
pub fn run(handles: Handles, launched: Instant) -> iced::Result {
    let slot = Mutex::new(Some(handles));
    let mut app = iced::application(
        move || {
            let handles = slot
                .lock()
                .ok()
                .and_then(|mut slot| slot.take())
                .expect("the application boots once");
            App::boot(handles, launched)
        },
        App::update,
        App::view,
    )
    .title(App::title)
    .subscription(App::subscription)
    .theme(App::theme)
    .default_font(theme::ui())
    .antialiasing(true)
    .window_size(Size::new(1320.0, 860.0));
    for font in theme::font_files() {
        app = app.font(font);
    }
    app.run()
}

impl App {
    pub(crate) fn boot(handles: Handles, launched: Instant) -> (Self, Task<Message>) {
        let now = Instant::now();
        let app = Self {
            model: Model::default(),
            events: handles.events,
            input: text_editor::Content::new(),
            history: InputHistory::new(),
            mode: InputMode::Chat,
            side: Some(SideTab::Agents),
            overlay: None,
            confirm: None,
            visible: PAGE,
            started: now,
            now,
            width: 1320.0,
            editing: None,
            session_ended: false,
            perf: Perf::new(launched),
        };
        let commands = Task::run(command_batches(handles.commands), |m| m);
        (app, Task::batch([commands, operation::focus(INPUT_ID)]))
    }

    fn title(&self) -> String {
        match self.model.active_agent() {
            Some(agent) => match &agent.project {
                Some(project) if !project.is_empty() => format!("AURA — {project}"),
                _ => format!("AURA — {}", agent.name),
            },
            None => "AURA".to_string(),
        }
    }

    fn theme(&self) -> Theme {
        theme::theme()
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            keyboard::listen().map(Message::Key),
            event::listen_with(window_event),
        ];
        if self.model.is_animating() {
            subs.push(window::frames().map(Message::Tick));
        }
        if self.model.toast.is_some() {
            subs.push(iced::time::every(Duration::from_millis(500)).map(Message::Toasts));
        }
        Subscription::batch(subs)
    }

    fn view(&self) -> Element<'_, Message> {
        let started = Instant::now();
        let element = view::view(self);
        self.perf.view(started.elapsed());
        element
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let started = Instant::now();
        let task = self.handle(message);
        self.perf.update(started.elapsed());
        task
    }

    /// Seconds since boot, for animations.
    #[must_use]
    pub fn clock(&self) -> f32 {
        self.now.duration_since(self.started).as_secs_f32()
    }

    #[must_use]
    pub fn busy(&self) -> bool {
        self.model.phase != Phase::Idle
    }

    fn send(&self, event: UiEvent) {
        if let Err(e) = self.events.try_send(event) {
            tracing::warn!(error = %e, "agent event loop did not take a UI event");
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Commands(batch) => {
                let was_empty = self.model.entries.is_empty();
                let effects: Vec<_> = batch
                    .into_iter()
                    .filter_map(|cmd| self.model.apply(cmd))
                    .collect();
                let mut tasks: Vec<_> = effects.into_iter().map(|e| self.effect(e)).collect();
                if self.model.link_prompt.is_some()
                    && !matches!(self.overlay, Some(Overlay::LinkCode { .. }))
                {
                    self.overlay = Some(Overlay::LinkCode {
                        code: String::new(),
                    });
                }
                if was_empty && !self.model.entries.is_empty() {
                    tasks.push(operation::snap_to_end(TRANSCRIPT_ID));
                }
                return Task::batch(tasks);
            }
            Message::SessionEnded => {
                self.session_ended = true;
                self.model.push_system(
                    "The agent session ended. Restart AURA to continue.",
                    Level::Error,
                );
            }
            Message::Input(action) => {
                if action.is_edit() {
                    self.history.reset();
                }
                self.input.perform(action);
            }
            Message::Submit => return self.submit(),
            Message::HistoryPrev => {
                if let Some(entry) = self.history.previous().map(str::to_string) {
                    self.set_input(&entry);
                }
            }
            Message::HistoryNext => {
                let entry = self.history.next_newer().unwrap_or_default().to_string();
                self.set_input(&entry);
            }
            Message::CompleteSlash => {
                if let Some(first) = slash::matching(self.input.text().trim_end()).first() {
                    return self.handle(Message::PickSlash(first.name));
                }
            }
            Message::PickSlash(name) => {
                self.set_input(&format!("/{name} "));
                return operation::focus(INPUT_ID);
            }
            Message::Escape => {
                if self.confirm.take().is_some() || self.overlay.take().is_some() {
                    return operation::focus(INPUT_ID);
                }
                if self.busy() {
                    return self.handle(Message::Cancel);
                }
            }
            Message::Cancel => {
                if self.busy() {
                    self.send(UiEvent::Cancel);
                    self.model.status = "Cancelling...".to_string();
                }
            }
            Message::SetMode(mode) => {
                self.mode = mode;
                return operation::focus(INPUT_ID);
            }
            Message::Side(tab) => {
                if self.side == Some(tab) {
                    self.side = None;
                } else {
                    self.side = Some(tab);
                    match tab {
                        SideTab::Agents => self.send(UiEvent::RefreshAgents),
                        SideTab::Files => self.model.sandbox.refresh(),
                        SideTab::Records | SideTab::Processes => {}
                    }
                }
            }
            Message::ToggleEntry(id) => self.model.toggle(id),
            Message::ToggleExpandAll => {
                let expand = !self.model.expand_activity;
                self.model.expand_activity = expand;
                for entry in &mut self.model.entries {
                    if entry.is_activity() {
                        entry.expanded = expand;
                    }
                }
            }
            Message::ShowEarlier => self.visible += PAGE,
            Message::Copy(text) => {
                self.model.toast("Copied to clipboard", Level::Success);
                return iced::clipboard::write(text);
            }
            Message::LinkClicked(url) => open_external(&url),
            Message::Approve | Message::Deny => {
                let approved = matches!(message, Message::Approve);
                if let Some(approval) = self.model.resolve_approval(approved) {
                    self.send(if approved {
                        UiEvent::Approve(approval.id)
                    } else {
                        UiEvent::Deny(approval.id)
                    });
                }
                return operation::focus(INPUT_ID);
            }
            Message::OpenModels => {
                if self.model.models.is_empty() {
                    self.model
                        .toast("Model list not loaded yet", Level::Warning);
                } else {
                    self.overlay = Some(Overlay::Models);
                }
            }
            Message::ChooseModel(id) => return self.choose_model(&id),
            Message::OpenHelp => self.overlay = Some(Overlay::Help),
            Message::CloseOverlay => {
                self.overlay = None;
                return operation::focus(INPUT_ID);
            }
            Message::LoginEmail(value) => {
                if let Some(Overlay::Login { email, .. }) = &mut self.overlay {
                    *email = value;
                }
            }
            Message::LoginPassword(value) => {
                if let Some(Overlay::Login { password, .. }) = &mut self.overlay {
                    *password = value;
                }
            }
            Message::LoginSubmit => {
                if let Some(Overlay::Login { email, password }) = &self.overlay {
                    if !email.trim().is_empty() && !password.is_empty() {
                        self.send(UiEvent::LoginCredentials {
                            email: email.trim().to_string(),
                            password: password.clone(),
                        });
                        self.model.status = "Signing in...".to_string();
                        self.overlay = None;
                    }
                }
            }
            Message::LinkCodeChanged(value) => {
                if let Some(Overlay::LinkCode { code }) = &mut self.overlay {
                    *code = value;
                }
            }
            Message::LinkCodeSubmit => {
                if let Some(Overlay::LinkCode { code }) = &self.overlay {
                    if !code.trim().is_empty() {
                        self.send(UiEvent::LinkCode(code.trim().to_string()));
                        self.model.link_prompt = None;
                        self.overlay = None;
                    }
                }
            }
            Message::OpenProject(purpose) => {
                let name = match purpose {
                    ProjectPurpose::Rename => self
                        .model
                        .active_agent()
                        .and_then(|a| a.project.clone())
                        .unwrap_or_default(),
                    ProjectPurpose::Create => String::new(),
                };
                let folder = std::env::current_dir()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default();
                self.overlay = Some(Overlay::Project {
                    purpose,
                    name,
                    folder,
                });
            }
            Message::ProjectName(value) => {
                if let Some(Overlay::Project { name, .. }) = &mut self.overlay {
                    *name = value;
                }
            }
            Message::ProjectFolder(value) => {
                if let Some(Overlay::Project { folder, .. }) = &mut self.overlay {
                    *folder = value;
                }
            }
            Message::ProjectSubmit => return self.submit_project(),
            Message::SelectAgent(id) => {
                if self.busy() {
                    self.model.toast(
                        "Wait for the current turn to finish before switching agents",
                        Level::Warning,
                    );
                } else if id != self.model.active_agent {
                    self.send(UiEvent::SelectAgent(id));
                }
            }
            Message::NewAgent => {
                if self.busy() {
                    self.model.toast(
                        "Wait for the current turn to finish before creating an agent",
                        Level::Warning,
                    );
                } else {
                    self.send(UiEvent::CreateAgent(None));
                }
            }
            Message::Spawn(name) => self.send(UiEvent::InstantiateDefinition(name)),
            Message::AddToProject(name) => self.send(UiEvent::AddDefinitionToProject(name)),
            Message::EditDefinition(name) => self.send(UiEvent::EditDefinition(name)),
            Message::Ask(confirm) => self.confirm = Some(confirm),
            Message::ConfirmYes => match self.confirm.take() {
                Some(Confirm::DeleteDefinition(name)) => {
                    self.send(UiEvent::DeleteDefinition(name));
                }
                Some(Confirm::Kill(id)) => self.send(UiEvent::KillProcess(id)),
                None => {}
            },
            Message::ConfirmNo => self.confirm = None,
            Message::OpenRecord(seq) => self.overlay = Some(Overlay::Record(seq)),
            Message::SandboxRow(index) => {
                self.model.sandbox.select(index);
                let Some(row) = self.model.sandbox.rows().get(index).cloned() else {
                    return Task::none();
                };
                match row.kind {
                    SandboxEntryKind::Dir | SandboxEntryKind::More(_) => {
                        self.model.sandbox.toggle_selected();
                    }
                    SandboxEntryKind::File | SandboxEntryKind::Symlink => {
                        let path = row.path;
                        return Task::perform(read_file(path.clone()), move |text| {
                            Message::FileLoaded(path.clone(), text)
                        });
                    }
                }
            }
            Message::RefreshFiles => self.model.sandbox.refresh(),
            Message::FileLoaded(path, text) => self.overlay = Some(Overlay::File { path, text }),
            Message::EditorClosed(path) => self.send(UiEvent::DefinitionEdited(path)),
            Message::DoneEditing => {
                if let Some(path) = self.editing.take() {
                    self.send(UiEvent::DefinitionEdited(path));
                }
            }
            Message::Key(event) => return self.key(event),
            Message::Tick(now) => {
                self.now = now;
                self.perf.frame(now);
            }
            Message::Toasts(now) => self.model.expire_toast(now),
            Message::Resized(size) => self.width = size.width,
        }
        Task::none()
    }

    fn set_input(&mut self, text: &str) {
        self.input = text_editor::Content::with_text(text);
        self.input
            .perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
    }

    fn effect(&mut self, effect: Effect) -> Task<Message> {
        match effect {
            Effect::OpenEditor(path) => self.open_editor(path),
            Effect::ChooseModel(id) => self.choose_model(&id),
            Effect::RefreshSandbox => {
                if self.side == Some(SideTab::Files) {
                    self.model.sandbox.refresh();
                }
                Task::none()
            }
        }
    }

    fn submit(&mut self) -> Task<Message> {
        let text = self.input.text();
        let text = text.trim_end_matches(['\n', '\r']).to_string();
        if text.trim().is_empty() {
            return Task::none();
        }
        self.input = text_editor::Content::new();
        self.history.add(&text);

        let shell = match self.mode {
            InputMode::Shell => Some(text.trim()),
            InputMode::Chat => text.strip_prefix('!').map(str::trim),
        };
        if let Some(command) = shell {
            if command.is_empty() {
                return Task::none();
            }
            let command = command.to_string();
            self.model.send_command(&command);
            self.send(UiEvent::RunCommand {
                command,
                cols: self.command_columns(),
            });
        } else if text.starts_with('/') {
            return self.slash(&text);
        } else {
            self.model.send_user(&text);
            self.send(UiEvent::UserMessage(text));
        }
        operation::snap_to_end(TRANSCRIPT_ID)
    }

    /// Columns of mono text that fit in the transcript, for command output.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn command_columns(&self) -> u16 {
        let side = if self.side.is_some() { SIDE_WIDTH } else { 0.0 };
        let usable = (self.width - side - 120.0).max(0.0);
        let cell = theme::MONO_SIZE * theme::MONO_ADVANCE_EM;
        (usable / cell).floor().clamp(20.0, 500.0) as u16
    }

    fn submit_project(&mut self) -> Task<Message> {
        let Some(Overlay::Project {
            purpose,
            name,
            folder,
        }) = &self.overlay
        else {
            return Task::none();
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            return Task::none();
        }
        match purpose {
            ProjectPurpose::Rename => self.send(UiEvent::RenameProject(name)),
            ProjectPurpose::Create => {
                let folder = PathBuf::from(folder.trim());
                if !folder.is_dir() {
                    self.model.toast(
                        format!("Not a folder: {}", folder.display()),
                        Level::Warning,
                    );
                    return Task::none();
                }
                self.send(UiEvent::CreateProject { name, folder });
            }
        }
        self.overlay = None;
        operation::focus(INPUT_ID)
    }

    /// Select `id` for the next turn; models whose sign-in is missing
    /// start that sign-in first and are selected once it succeeds.
    fn choose_model(&mut self, id: &str) -> Task<Message> {
        let missing = self
            .model
            .models
            .iter()
            .find(|m| m.id == id && !m.available)
            .map(|m| m.provider.clone().unwrap_or_default());
        if let Some(provider) = missing {
            self.model.pending_model = Some(id.to_string());
            match slash::plan_provider(&provider) {
                Some(plan) => self.start_link(plan, false),
                None => {
                    self.overlay = Some(Overlay::Login {
                        email: String::new(),
                        password: String::new(),
                    });
                    self.model
                        .toast("Cloud models need a zOS login", Level::Warning);
                }
            }
            return Task::none();
        }
        self.overlay = None;
        if id != self.model.current_model {
            self.send(UiEvent::SelectModel(id.to_string()));
        }
        operation::focus(INPUT_ID)
    }

    fn start_link(&mut self, provider: &str, paste: bool) {
        self.overlay = None;
        self.model.status = format!("Linking {provider}...");
        self.send(UiEvent::LinkSubscription {
            provider: provider.to_string(),
            paste,
        });
    }

    #[allow(clippy::too_many_lines)]
    fn slash(&mut self, text: &str) -> Task<Message> {
        let mut parts = text[1..].splitn(2, ' ');
        let cmd = parts.next().unwrap_or("").to_ascii_lowercase();
        let arg = parts.next().unwrap_or("").trim().to_string();
        match cmd.as_str() {
            "quit" | "exit" | "q" => {
                self.send(UiEvent::Quit);
                return iced::exit();
            }
            "help" | "?" => self.overlay = Some(Overlay::Help),
            "clear" => {
                self.model.entries.clear();
                self.send(UiEvent::Clear);
            }
            "status" | "s" => self.send(UiEvent::ShowStatus),
            "history" | "h" => self.send(UiEvent::ShowHistory(None)),
            "records" | "record" | "kernel" | "r" | "k" => self.side = Some(SideTab::Records),
            "processes" | "ps" => self.side = Some(SideTab::Processes),
            "files" | "sandbox" | "f" => {
                self.side = Some(SideTab::Files);
                self.model.sandbox.refresh();
            }
            "agents" | "swarm" | "sw" => {
                self.side = Some(SideTab::Agents);
                self.send(UiEvent::RefreshAgents);
            }
            "agent" | "a" => self.agent_command(&arg),
            "project" => return self.handle(Message::OpenProject(ProjectPurpose::Create)),
            "name" => {
                if arg.is_empty() {
                    return self.handle(Message::OpenProject(ProjectPurpose::Rename));
                }
                self.send(UiEvent::RenameProject(arg));
            }
            "spawn" => {
                let name = self
                    .model
                    .definitions
                    .iter()
                    .find(|d| d.name.eq_ignore_ascii_case(&arg))
                    .map(|d| d.name.clone());
                match name {
                    Some(name) => self.send(UiEvent::InstantiateDefinition(name)),
                    None => {
                        let names: Vec<_> = self
                            .model
                            .definitions
                            .iter()
                            .map(|d| d.name.as_str())
                            .collect();
                        let usage =
                            format!("Usage: /spawn <permanent agent> ({})", names.join(", "));
                        self.model.toast(usage, Level::Warning);
                    }
                }
            }
            "login" => {
                let mut words = arg.split_whitespace();
                let target = words.next().unwrap_or("");
                let paste = words
                    .next()
                    .is_some_and(|w| w.eq_ignore_ascii_case("paste"));
                if target.is_empty() || target.eq_ignore_ascii_case("zos") {
                    self.overlay = Some(Overlay::Login {
                        email: String::new(),
                        password: String::new(),
                    });
                } else if let Some(provider) = slash::plan_provider(target) {
                    self.start_link(provider, paste);
                } else {
                    self.model.toast(
                        format!("Unknown provider: {target}. Use zos, claude or chatgpt."),
                        Level::Warning,
                    );
                }
            }
            "logout" => match slash::plan_provider(&arg) {
                Some(provider) => self.send(UiEvent::Unlink(provider.to_string())),
                None if arg.is_empty() || arg.eq_ignore_ascii_case("zos") => {
                    self.send(UiEvent::Logout);
                }
                None => self.model.toast(
                    format!("Unknown provider: {arg}. Use zos, claude or chatgpt."),
                    Level::Warning,
                ),
            },
            "whoami" | "me" => self.send(UiEvent::Whoami),
            "model" | "models" | "m" => {
                if arg.is_empty() {
                    return self.handle(Message::OpenModels);
                }
                let id = self
                    .model
                    .models
                    .iter()
                    .find(|m| m.id.eq_ignore_ascii_case(&arg) || m.label.eq_ignore_ascii_case(&arg))
                    .map_or_else(|| arg.clone(), |m| m.id.clone());
                return self.choose_model(&id);
            }
            "new" | "n" => {
                self.model.entries.clear();
                self.send(UiEvent::NewSession);
                self.model.toast("New session started", Level::Success);
            }
            "terminal" | "shell" | "t" => self.mode = self.mode.other(),
            "expand" => return self.handle(Message::ToggleExpandAll),
            _ => self.model.toast(
                format!("Unknown command: /{cmd}. Type /help for available commands."),
                Level::Warning,
            ),
        }
        Task::none()
    }

    fn agent_command(&mut self, arg: &str) {
        let mut parts = arg.splitn(2, ' ');
        let first = parts.next().unwrap_or("");
        if first.eq_ignore_ascii_case("new") {
            if self.busy() {
                self.model.toast(
                    "Wait for the current turn to finish before creating an agent",
                    Level::Warning,
                );
                return;
            }
            let name = parts
                .next()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string);
            self.send(UiEvent::CreateAgent(name));
            return;
        }
        if arg.is_empty() {
            self.model.toast(
                "Usage: /agent new [name] | /agent <number|name>",
                Level::Warning,
            );
            return;
        }
        let target = arg
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| self.model.agents.get(i))
            .or_else(|| {
                self.model
                    .agents
                    .iter()
                    .find(|a| a.name.eq_ignore_ascii_case(arg))
            })
            .map(|a| a.id.clone());
        match target {
            Some(id) => {
                let _ = self.handle(Message::SelectAgent(id));
            }
            None => self
                .model
                .toast(format!("No agent matching '{arg}'"), Level::Warning),
        }
    }

    /// Shortcuts that work wherever the focus is. Keys the chat input
    /// handled itself never get here.
    fn key(&mut self, event: keyboard::Event) -> Task<Message> {
        let keyboard::Event::KeyPressed {
            key,
            modifiers,
            text,
            ..
        } = event
        else {
            return Task::none();
        };
        if modifiers.command() {
            let message = match key.as_ref() {
                Key::Character("k") => Some(Message::OpenModels),
                Key::Character("b") => Some(Message::Side(self.side.unwrap_or(SideTab::Agents))),
                Key::Character("1") => Some(Message::Side(SideTab::Agents)),
                Key::Character("2") => Some(Message::Side(SideTab::Records)),
                Key::Character("3") => Some(Message::Side(SideTab::Processes)),
                Key::Character("4") => Some(Message::Side(SideTab::Files)),
                Key::Character("o") => Some(Message::ToggleExpandAll),
                Key::Character("/") => Some(Message::OpenHelp),
                Key::Character("j" | "`") => Some(Message::SetMode(self.mode.other())),
                Key::Character("c") if self.busy() => Some(Message::Cancel),
                _ => None,
            };
            return message.map_or_else(Task::none, |m| self.handle(m));
        }
        match key.as_ref() {
            Key::Named(Named::Escape) => self.handle(Message::Escape),
            Key::Character("y") if self.model.approval.is_some() => self.handle(Message::Approve),
            Key::Character("n") if self.model.approval.is_some() => self.handle(Message::Deny),
            _ if self.overlay.is_none()
                && text
                    .as_deref()
                    .is_some_and(|t| !t.is_empty() && !t.chars().any(char::is_control)) =>
            {
                // Typing while nothing has focus goes to the chat input.
                for c in text.as_deref().unwrap_or_default().chars() {
                    self.input
                        .perform(text_editor::Action::Edit(text_editor::Edit::Insert(c)));
                }
                operation::focus(INPUT_ID)
            }
            _ => Task::none(),
        }
    }

    fn open_editor(&mut self, path: PathBuf) -> Task<Message> {
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .ok()
            .filter(|e| !e.trim().is_empty() && !is_terminal_editor(e));
        let Some(editor) = editor else {
            open_external(&path.display().to_string());
            self.model.toast(
                "Opened in your editor. Save it, then press Done editing.",
                Level::Info,
            );
            self.editing = Some(path);
            return Task::none();
        };
        self.model.status = format!("Editing {}", path.display());
        let target = path.clone();
        Task::perform(
            async move {
                let mut words = editor.split_whitespace();
                let program = words.next().unwrap_or_default().to_string();
                let status = tokio::process::Command::new(program)
                    .args(words)
                    .arg(&target)
                    .status()
                    .await;
                if let Err(e) = status {
                    tracing::warn!(error = %e, "editor did not start");
                }
            },
            move |()| Message::EditorClosed(path.clone()),
        )
    }
}

/// Commands from the event loop, grouped so a burst is applied in one update.
fn command_batches(commands: mpsc::Receiver<UiCommand>) -> impl Stream<Item = Message> {
    futures::stream::unfold(Some(commands), |commands| async move {
        let mut commands = commands?;
        match commands.recv().await {
            Some(first) => {
                let mut batch = vec![first];
                while batch.len() < MAX_BATCH {
                    match commands.try_recv() {
                        Ok(cmd) => batch.push(cmd),
                        Err(_) => break,
                    }
                }
                Some((Message::Commands(batch), Some(commands)))
            }
            None => Some((Message::SessionEnded, None)),
        }
    })
}

#[allow(clippy::needless_pass_by_value)] // the signature `event::listen_with` takes
fn window_event(event: iced::Event, _status: event::Status, _id: window::Id) -> Option<Message> {
    match event {
        iced::Event::Window(window::Event::Resized(size)) => Some(Message::Resized(size)),
        iced::Event::Window(window::Event::Opened { size, .. }) => Some(Message::Resized(size)),
        _ => None,
    }
}

async fn read_file(path: PathBuf) -> Result<String, String> {
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "{} KB is too large to show here (limit {} KB)",
            meta.len() / 1024,
            MAX_FILE_BYTES / 1024
        ));
    }
    let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|_| "Binary file".to_string())
}

/// Editors that need a terminal of their own, which a desktop window
/// doesn't have.
fn is_terminal_editor(editor: &str) -> bool {
    let program = editor.split_whitespace().next().unwrap_or_default();
    let name = std::path::Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);
    matches!(
        name,
        "vi" | "vim"
            | "nvim"
            | "nano"
            | "pico"
            | "micro"
            | "hx"
            | "helix"
            | "kak"
            | "ed"
            | "joe"
            | "mg"
    ) || (name == "emacs" && editor.contains("-nw"))
}

/// Open a URL or file with the platform's default handler.
fn open_external(target: &str) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(target).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(target)
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(target).spawn();
    if let Err(e) = result {
        tracing::warn!(error = %e, target, "could not open");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_editors_are_recognised() {
        assert!(is_terminal_editor("nvim"));
        assert!(is_terminal_editor("/usr/bin/vim -u NONE"));
        assert!(is_terminal_editor("emacs -nw"));
        assert!(!is_terminal_editor("code --wait"));
        assert!(!is_terminal_editor("emacs"));
    }
}
