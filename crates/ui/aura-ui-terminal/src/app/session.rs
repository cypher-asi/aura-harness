//! Agent, model and account details, chat width and spinners.

use super::*;

impl App {
    /// File tree of the active agent's sandbox.
    #[must_use]
    pub const fn sandbox(&self) -> &SandboxTree {
        &self.sandbox
    }

    /// Get the active agent ID.
    #[must_use]
    pub fn active_agent_id(&self) -> &str {
        &self.active_agent_id
    }

    /// Check if the API is active.
    #[must_use]
    pub const fn api_active(&self) -> bool {
        self.api_active
    }

    /// Models offered by the selector.
    #[must_use]
    pub fn models(&self) -> &[ModelOptionView] {
        &self.models
    }

    /// Id of the model used for the next turn (empty until the kernel reports it).
    #[must_use]
    pub fn current_model(&self) -> &str {
        &self.current_model
    }

    /// The model and route Gate sends the next turn to, `None` for a
    /// pinned model.
    #[must_use]
    pub fn model_route(&self) -> Option<&str> {
        self.model_route.as_deref()
    }

    /// Thinking level pinned for every turn (`high`, `off`, ...), `None`
    /// when each model picks its own.
    #[must_use]
    pub fn thinking(&self) -> Option<&str> {
        self.thinking.as_deref()
    }

    /// Selector entry for the current model, if listed.
    #[must_use]
    pub fn current_model_option(&self) -> Option<&ModelOptionView> {
        self.models.iter().find(|m| m.id == self.current_model)
    }

    /// Logged-in user's display name (`None` = guest).
    #[must_use]
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Labels of the usable linked plans (e.g. "Claude").
    #[must_use]
    pub fn plans(&self) -> &[String] {
        &self.plans
    }

    /// Work mode of the agent on screen.
    #[must_use]
    pub fn mode(&self) -> crate::mode_command::PermissionMode {
        self.hud.mode.unwrap_or_default()
    }

    /// Approval level of the agent on screen; `None` until the event loop
    /// reports it.
    #[must_use]
    pub const fn approval(&self) -> Option<crate::mode_command::ApprovalLevel> {
        self.hud.approval
    }

    /// The active agent's current plan.
    #[must_use]
    pub const fn active_plan(&self) -> Option<&PlanView> {
        self.active_plan.as_ref()
    }

    /// Bash-style `[user@host dir]$ ` prompt shown before the input line at
    /// the terminal prompt.
    #[must_use]
    pub fn shell_prompt(&self) -> &str {
        &self.shell_prompt
    }

    #[cfg(test)]
    pub(crate) fn set_shell_prompt(&mut self, prompt: &str) {
        self.shell_prompt = prompt.to_string();
    }

    /// Whether the input line shows the terminal prompt and Enter runs it
    /// as a shell command. Prompts that ask for a value (login, folder)
    /// take the line over while they are open.
    #[must_use]
    pub const fn input_is_command(&self) -> bool {
        self.terminal_mode
            && !matches!(
                self.state,
                AppState::LoginEmail
                    | AppState::LoginPassword
                    | AppState::LinkCode
                    | AppState::SelectingFolder
                    | AppState::ChannelForm
            )
    }

    /// Record the width of the chat text area drawn this frame.
    pub fn set_chat_width(&mut self, width: u16) {
        self.chat_width = width;
    }

    /// Width of the chat text area drawn last frame.
    #[cfg(test)]
    pub(crate) const fn chat_width(&self) -> u16 {
        self.chat_width
    }

    /// Terminal width for a command, matching where its output is drawn.
    pub(crate) fn command_columns(&self) -> u16 {
        if self.chat_width == 0 {
            return DEFAULT_COMMAND_COLUMNS;
        }
        self.chat_width.max(MIN_COMMAND_COLUMNS)
    }

    /// Model selector state (tab, filter, highlighted row).
    #[must_use]
    pub const fn model_picker(&self) -> &ModelPicker {
        &self.model_picker
    }

    /// Where the model selector and its rows were drawn this frame.
    pub fn set_model_picker_layout(&mut self, layout: PickerLayout) {
        self.model_picker_layout = layout;
    }

    /// Get the current spinner character for animations.
    #[must_use]
    pub fn spinner_char(&self) -> &'static str {
        const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        SPINNER_FRAMES[self.animation_frame % SPINNER_FRAMES.len()]
    }

    /// Frames drawn so far, to step animations by.
    #[must_use]
    pub const fn animation_frame(&self) -> usize {
        self.animation_frame
    }

    /// Current frame of the ring shown beside thinking agents.
    #[must_use]
    pub fn aura_frame(&self) -> (String, ratatui::style::Color) {
        crate::animation::aura_ring(self.animation_frame)
    }

    /// Get the records list.
    #[must_use]
    pub const fn records(&self) -> &VecDeque<RecordSummary> {
        &self.records
    }

    /// Get the selected record index.
    #[must_use]
    pub const fn selected_record(&self) -> usize {
        self.selected_record
    }

    /// Get the records scroll offset.
    #[must_use]
    pub const fn records_scroll(&self) -> usize {
        self.records_scroll
    }
}
