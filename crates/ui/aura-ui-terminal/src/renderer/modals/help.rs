//! The help dialog: the commands, the keys, and the keys of the dialogs,
//! which show none themselves. It scrolls when the screen is too short
//! for it.

use super::with_notice;
use crate::components::{Info, ListScroll, Modal, ModalArea, Value};
use crate::{App, Theme};
use ratatui::{text::Line, Frame};

/// Columns of the keys, before what they do.
const KEYS_WIDTH: usize = 24;

/// The commands.
const COMMANDS: [(&str, &str); 30] = [
    ("/help", "Show this help"),
    ("/new", "New session (reset context)"),
    ("/resume [n]", "List or resume earlier sessions"),
    ("/rewind [n] [code|chat]", "Undo to before turn n"),
    ("/diff [session]", "Files changed by last turn"),
    ("/gate /compare", "Gate's table, A/B run"),
    ("/clear", "Clear messages"),
    ("/context", "What fills the context window"),
    ("/compact [focus]", "Summarize older history now"),
    ("/init", "Write or improve AGENTS.md"),
    ("/todos", "Fold or open the to-do drawer"),
    ("/cost", "Tokens and cost of the session"),
    ("/agent [n|name]", "Add an agent, or switch"),
    ("/agent new [name]", "Create and switch to agent"),
    ("/project", "The project's folder and agents"),
    ("/archive", "Hide the agent"),
    ("/remove-project", "Hide the project"),
    ("/agents /kernel /files", "Toggle those panels"),
    ("/graph [id]", "A run's graph"),
    ("/runs /status", "Runs, scores"),
    ("/model [opus]", "Pick the model"),
    ("/thinking [level]", "Thinking level"),
    ("/mode [name]", "How the agent works"),
    ("/approval [level]", "What asks for approval"),
    ("/plan /build [path]", "Plan mode, implement plan"),
    ("/login /whoami", "Login for cloud models, status"),
    ("/login <plan> [paste]", "Link a claude or chatgpt plan"),
    ("/logout [plan]", "Sign out / unlink"),
    ("/expand", "Chat panel full screen"),
    ("/quit", "Exit"),
];

/// The keys.
const KEYS: [(&str, &str); 20] = [
    ("Enter / Shift+Enter", "Send / new line"),
    ("@path", "Attach a file or folder"),
    ("Ctrl+V", "Paste an image (or text)"),
    ("Ctrl+G", "Edit the input in $EDITOR"),
    ("$ / Esc", "Open the terminal / back to chat"),
    ("Tab", "Switch panels"),
    ("Shift+Tab", "Approval level (yolo→ask→strict)"),
    ("↑/↓", "Navigate / History"),
    ("Ctrl+N", "New project (pick its folder)"),
    ("Space / Shift+Space", "Fold agent / its project"),
    ("Shift+←/→", "Panel tabs: TERMINAL, GRAPH"),
    ("Ctrl+X", "Chat panel full screen"),
    ("o / k / l", "Process: open agent, kill, log"),
    ("Alt+↑/↓/1..5 Alt+T", "Agent, app, thinking level"),
    ("Mouse", "Wheel/click agents to switch"),
    ("PgUp/PgDn Ctrl+Home/End", "Scroll chat, to end"),
    ("Ctrl+O", "Expand/collapse thinking & tools"),
    ("Click", "A summary, then a step's details"),
    ("Shift+Mouse", "Select text to copy"),
    ("Ctrl+C / Ctrl+T", "Cancel or exit / stop a command"),
];

/// The keys of the dialogs, which show none themselves.
const DIALOG_KEYS: [(&str, &str); 4] = [
    ("Esc, click outside", "Close; deny an approval"),
    ("Type, Tab", "Models: filter, switch tab"),
    ("n ~ . Space", "Folders: new, home, hidden, pick"),
    ("←/→ Ctrl+Enter", "Channel: project, submit"),
];

/// Draw the help, from the line it is scrolled to.
pub(super) fn render(frame: &mut Frame, app: &mut App, theme: &Theme) -> ModalArea {
    let screen = frame.area();
    let modal = with_notice(Modal::new("HELP", theme), app, theme);
    let lines = help_lines(modal.body_width(screen), theme);
    let total = lines.len();
    let visible = modal.body_room(screen).min(total);
    let first = app.help_scroll().min(total - visible);
    app.set_help_scroll(first);
    let modal = modal.scroll(ListScroll {
        top: 0,
        visible,
        total,
        first,
    });
    modal.render(
        frame.buffer_mut(),
        screen,
        lines.into_iter().skip(first).collect(),
    )
}

/// Every section of the help, `width` columns wide.
fn help_lines(width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut info = Info::new(width.max(1), theme)
        .label_width(KEYS_WIDTH)
        .wrap_values();
    let sections: [(&str, &[(&str, &str)]); 3] = [
        ("COMMANDS", &COMMANDS),
        ("KEYS", &KEYS),
        ("IN DIALOGS", &DIALOG_KEYS),
    ];
    for (title, entries) in sections {
        info.section(title, None);
        for (keys, what) in entries {
            info.fact(keys, what, Value::Plain);
        }
    }
    info.into_lines()
}
