//! Message component for chat bubbles.

use crate::themes::Theme;
use chrono::{DateTime, Local, Utc};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::app::image_preview::ImagePreview;
use crate::components::{Activity, BootConsole, LiveOutput};
use crate::events::{CallUsage, FileDiffData, MessageRole, SourceUsage};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static NEXT_MESSAGE_ID: AtomicU64 = AtomicU64::new(0);

fn next_message_id() -> u64 {
    NEXT_MESSAGE_ID.fetch_add(1, Ordering::Relaxed)
}

/// A chat message.
#[derive(Debug)]
pub struct Message {
    /// Message role
    role: MessageRole,
    /// Message content
    content: String,
    /// Timestamp
    timestamp: DateTime<Utc>,
    /// Whether the message is still streaming
    is_streaming: bool,
    /// Set when the message is a shell command the user ran
    shell: Option<ShellRun>,
    /// Set when the message is a step the agent took (thinking, a tool call)
    activity: Option<Activity>,
    /// Whether an activity block shows its full detail
    expanded: bool,
    /// Model that wrote an assistant reply
    model: Option<String>,
    /// Tokens and cost of the model call that wrote an assistant reply
    usage: Option<CallUsage>,
    /// Those tokens split by the sign-in that paid for them
    sources: Vec<SourceUsage>,
    /// Set when the message shows file changes
    diff: Option<DiffBlock>,
    /// Set when the message is a boot console
    boot: Option<BootConsole>,
    /// Images the user pasted into the message, drawn under its text
    images: Vec<Arc<ImagePreview>>,
    /// Who wrote it, when it is a post of a channel: drawn above its text
    author: Option<String>,
    /// Unique to this message, clones included
    id: u64,
    /// Bumped on every change
    revision: u64,
}

impl Clone for Message {
    fn clone(&self) -> Self {
        Self {
            role: self.role,
            content: self.content.clone(),
            timestamp: self.timestamp,
            is_streaming: self.is_streaming,
            shell: self.shell.clone(),
            activity: self.activity.clone(),
            expanded: self.expanded,
            model: self.model.clone(),
            usage: self.usage,
            sources: self.sources.clone(),
            diff: self.diff.clone(),
            boot: self.boot.clone(),
            images: self.images.clone(),
            author: self.author.clone(),
            id: next_message_id(),
            revision: 0,
        }
    }
}

/// File changes shown as a chat message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBlock {
    /// What the changes are, e.g. which turn made them
    pub title: String,
    /// One entry per changed file
    pub files: Vec<FileDiffData>,
}

/// A shell command typed by the user and its terminal output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellRun {
    /// Command line as typed
    pub command: String,
    /// Terminal output (may contain ANSI styling); `None` while running
    pub output: Option<String>,
    /// Exit code of a failed command
    pub exit_code: Option<i32>,
    /// What the command has printed so far, while it runs
    pub live: LiveOutput,
}

impl Message {
    /// Create a new message.
    #[must_use]
    pub fn new(role: MessageRole, content: &str) -> Self {
        Self {
            role,
            content: content.to_string(),
            timestamp: Utc::now(),
            is_streaming: false,
            shell: None,
            activity: None,
            expanded: false,
            model: None,
            usage: None,
            sources: Vec::new(),
            diff: None,
            boot: None,
            images: Vec::new(),
            author: None,
            id: next_message_id(),
            revision: 0,
        }
    }

    /// A post of a channel by `author`, shown as a message of `role`. Its
    /// id is `id` rather than a fresh one, so a post rebuilt every frame
    /// keeps the lines laid out for it; `revision` says what it was built
    /// from.
    #[must_use]
    pub(crate) fn post(
        (id, revision): (u64, u64),
        role: MessageRole,
        author: String,
        (content, timestamp): (&str, DateTime<Utc>),
    ) -> Self {
        Self {
            author: Some(author),
            timestamp,
            id,
            revision,
            ..Self::new(role, content)
        }
    }

    /// Who wrote the message, when it is a post of a channel.
    #[must_use]
    pub fn author(&self) -> Option<&str> {
        self.author.as_deref()
    }

    /// File changes, drawn as one diff per file under `title`.
    #[must_use]
    pub fn diff(title: &str, files: Vec<FileDiffData>) -> Self {
        let mut message = Self::new(MessageRole::System, title);
        message.diff = Some(DiffBlock {
            title: title.to_string(),
            files,
        });
        message
    }

    /// The message with `images` drawn under its text.
    #[must_use]
    pub fn with_images(mut self, images: Vec<Arc<ImagePreview>>) -> Self {
        self.images = images;
        self.touch();
        self
    }

    /// Images pasted into the message.
    #[must_use]
    pub fn images(&self) -> &[Arc<ImagePreview>] {
        &self.images
    }

    /// The file changes this message shows, if it is a diff.
    #[must_use]
    pub const fn diff_block(&self) -> Option<&DiffBlock> {
        self.diff.as_ref()
    }

    /// A boot console, drawn as a collapsible block that shows everything
    /// while it runs.
    #[must_use]
    pub fn boot(console: BootConsole) -> Self {
        let mut message = Self::new(MessageRole::System, &console.title);
        message.boot = Some(console);
        message
    }

    /// The boot console this message shows, if it is one.
    #[must_use]
    pub const fn boot_console(&self) -> Option<&BootConsole> {
        self.boot.as_ref()
    }

    /// Change the boot console this message shows.
    pub fn update_boot(&mut self, update: impl FnOnce(&mut BootConsole)) {
        if let Some(console) = &mut self.boot {
            update(console);
            self.touch();
        }
    }

    /// Whether this is a boot console still going (drawn with a spinner
    /// and a running clock).
    #[must_use]
    pub fn is_live_boot(&self) -> bool {
        self.boot.as_ref().is_some_and(BootConsole::is_running)
    }

    /// A step the agent took, drawn as a collapsible block.
    #[must_use]
    pub fn activity(activity: Activity, expanded: bool) -> Self {
        let mut message = Self::new(MessageRole::Assistant, "");
        message.activity = Some(activity);
        message.expanded = expanded;
        message
    }

    /// The agent step this message shows, if it is one.
    #[must_use]
    pub const fn activity_step(&self) -> Option<&Activity> {
        self.activity.as_ref()
    }

    /// Change the agent step this message shows.
    pub fn update_activity(&mut self, update: impl FnOnce(&mut Activity)) {
        if let Some(activity) = &mut self.activity {
            update(activity);
            self.touch();
        }
    }

    /// Whether this is an agent step still in progress (drawn with a
    /// spinner and a running clock).
    #[must_use]
    pub fn is_live_activity(&self) -> bool {
        self.activity.as_ref().is_some_and(Activity::is_running)
    }

    /// Whether an activity block shows its full detail.
    #[must_use]
    pub const fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// Show or hide an activity block's full detail.
    pub fn set_expanded(&mut self, expanded: bool) {
        if (self.activity.is_some() || self.boot.is_some()) && self.expanded != expanded {
            self.expanded = expanded;
            self.touch();
        }
    }

    /// `(id, revision)`: differs from every other message's, and changes
    /// whenever this message does.
    #[must_use]
    pub const fn version(&self) -> (u64, u64) {
        (self.id, self.revision)
    }

    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// A shell command the user ran; output is filled in by [`Self::finish_shell`].
    #[must_use]
    pub fn shell(command: &str) -> Self {
        let mut message = Self::new(MessageRole::User, command);
        message.shell = Some(ShellRun {
            command: command.to_string(),
            output: None,
            exit_code: None,
            live: LiveOutput::default(),
        });
        message
    }

    /// Add `chunk` to what a running shell command has printed so far.
    pub fn append_shell_output(&mut self, chunk: &str) {
        if let Some(run) = self.shell.as_mut().filter(|run| run.output.is_none()) {
            run.live.push(chunk);
            self.touch();
        }
    }

    /// The shell command this message shows, if it is one.
    #[must_use]
    pub const fn shell_run(&self) -> Option<&ShellRun> {
        self.shell.as_ref()
    }

    /// Whether this is a shell command still waiting for its output.
    #[must_use]
    pub fn is_running_shell(&self) -> bool {
        self.shell.as_ref().is_some_and(|run| run.output.is_none())
    }

    /// Record the output of a shell command message.
    pub fn finish_shell(&mut self, output: &str, exit_code: Option<i32>) {
        if let Some(run) = &mut self.shell {
            run.output = Some(output.to_string());
            run.exit_code = exit_code;
            run.live.clear();
            self.touch();
        }
    }

    /// Override the timestamp (e.g. when restoring history from the record).
    #[must_use]
    pub fn with_timestamp(mut self, timestamp: DateTime<Utc>) -> Self {
        self.timestamp = timestamp;
        self.touch();
        self
    }

    /// Model that wrote this reply, if known.
    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// Record the model that wrote this reply.
    pub fn set_model(&mut self, model: &str) {
        if self.model.as_deref() != Some(model) {
            self.model = Some(model.to_string());
            self.touch();
        }
    }

    /// Tokens and cost of the model call that wrote this reply, if known.
    #[must_use]
    pub const fn usage(&self) -> Option<CallUsage> {
        self.usage
    }

    /// Record the tokens and cost of the model call that wrote this reply.
    pub fn set_usage(&mut self, usage: Option<CallUsage>) {
        if self.usage != usage {
            self.usage = usage;
            self.touch();
        }
    }

    /// The reply's tokens split by the sign-in that paid for them.
    #[must_use]
    pub fn sources(&self) -> &[SourceUsage] {
        &self.sources
    }

    /// Record which sign-ins paid for the reply's tokens.
    pub fn set_sources(&mut self, sources: &[SourceUsage]) {
        if self.sources != sources {
            self.sources = sources.to_vec();
            self.touch();
        }
    }

    /// Get the message role.
    #[must_use]
    pub const fn role(&self) -> MessageRole {
        self.role
    }

    /// Get the message content.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Get the message timestamp formatted as local time (HH:MM:SS).
    #[must_use]
    pub fn timestamp_local(&self) -> String {
        let local_time: DateTime<Local> = DateTime::from(self.timestamp);
        local_time.format("%H:%M:%S").to_string()
    }

    /// When the message was written, in Unix milliseconds.
    #[must_use]
    pub fn timestamp_ms(&self) -> u64 {
        u64::try_from(self.timestamp.timestamp_millis()).unwrap_or(0)
    }

    /// Whether this is a reply that started streaming but has no text yet.
    #[must_use]
    pub fn is_empty_reply(&self) -> bool {
        self.is_streaming
            && self.content.is_empty()
            && self.activity.is_none()
            && self.shell.is_none()
    }

    /// Set whether the message is streaming.
    pub fn set_streaming(&mut self, streaming: bool) {
        self.is_streaming = streaming;
        self.touch();
    }

    /// Check if the message is still streaming.
    #[must_use]
    pub const fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    /// Append content to the message.
    pub fn append(&mut self, text: &str) {
        self.content.push_str(text);
        self.touch();
    }

    /// Set the message content (replaces existing content).
    pub fn set_content(&mut self, content: &str) {
        content.clone_into(&mut self.content);
        self.touch();
    }

    /// Render the message.
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let (label, label_color, border_style) = match self.role {
            MessageRole::User => ("YOU", theme.colors.foreground, Borders::ALL),
            MessageRole::Assistant => ("AURA", theme.colors.primary, Borders::ALL),
            MessageRole::System => ("SYSTEM", theme.colors.secondary, Borders::NONE),
        };

        // Format timestamp
        let local_time: DateTime<Local> = DateTime::from(self.timestamp);
        let timestamp = local_time.format("%H:%M:%S").to_string();

        // Build header line
        let mut header_spans = vec![
            Span::styled(
                format!("[{timestamp}] "),
                Style::default().fg(theme.colors.muted),
            ),
            Span::styled(
                label,
                Style::default()
                    .fg(label_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        // Streaming indicator
        if self.is_streaming && self.role == MessageRole::Assistant {
            header_spans.push(Span::styled(
                " ◇━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━▸",
                Style::default().fg(theme.colors.primary),
            ));
        }

        let header = Line::from(header_spans);

        // Build content lines
        let mut lines = vec![header, Line::from("")];
        for line in self.content.lines() {
            lines.push(Line::from(Span::styled(
                line,
                Style::default().fg(if self.role == MessageRole::Assistant {
                    theme.colors.primary
                } else {
                    theme.colors.foreground
                }),
            )));
        }

        let block = Block::default()
            .borders(border_style)
            .border_type(theme.border_style.to_border_type())
            .border_style(Style::default().fg(label_color));

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });

        frame.render_widget(paragraph, area);
    }

    /// Calculate the height needed to render this message.
    #[must_use]
    pub fn height(&self, width: u16) -> u16 {
        let content_lines = self.content.lines().count();
        let wrapped_lines = self
            .content
            .lines()
            .map(|line| {
                let line_len = u16::try_from(line.len()).unwrap_or(u16::MAX);
                let inner_width = width.saturating_sub(4); // Account for borders and padding
                if inner_width == 0 {
                    1
                } else {
                    (line_len / inner_width).max(1)
                }
            })
            .sum::<u16>();

        // Header + content + border padding
        let content_lines_u16 = u16::try_from(content_lines).unwrap_or(u16::MAX);
        2 + wrapped_lines.max(content_lines_u16) + 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_creation() {
        let msg = Message::new(MessageRole::User, "Hello, world!");
        assert_eq!(msg.role(), MessageRole::User);
        assert_eq!(msg.content(), "Hello, world!");
    }

    #[test]
    fn test_message_streaming() {
        let mut msg = Message::new(MessageRole::Assistant, "");
        assert!(!msg.is_streaming);
        msg.set_streaming(true);
        assert!(msg.is_streaming);
    }

    #[test]
    fn test_version_changes_with_content_and_differs_per_clone() {
        let mut msg = Message::new(MessageRole::Assistant, "Hello");
        let before = msg.version();
        msg.append("!");
        assert_ne!(msg.version(), before);
        assert_ne!(msg.clone().version().0, msg.version().0);
    }

    #[test]
    fn test_message_append() {
        let mut msg = Message::new(MessageRole::Assistant, "Hello");
        msg.append(", world!");
        assert_eq!(msg.content(), "Hello, world!");
    }
}
