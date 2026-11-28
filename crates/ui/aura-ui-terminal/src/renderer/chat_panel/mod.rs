//! The Chat panel: the conversation, the terminal, and the other chat
//! tabs under its header bar.

mod below;
mod links;
mod live_tail;
mod prose;
mod rows;
mod terminal;
mod text_area;
mod user_band;
mod visible;

use super::chrome::{render_body_block, render_scrollbar, scrollbar_track, ScrollView};
use super::detail::detail_header;
use super::header::{self, HeaderInfo, PanelHeader};
use super::side_list::render_empty;
use crate::{
    app::{
        scrollbar::{ScrollTarget, ScrollbarArea},
        tab_bar::TabLayout,
        ChatTab, HoverTarget, Pane, PanelFocus,
    },
    App, Theme,
};
use live_tail::{live_tail, LiveTail};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use rows::{render_chat_lines, ChatRows};

pub(crate) use live_tail::format_elapsed;
pub(super) use prose::{pack_prose, prose_columns};
pub(super) use text_area::{breathing_room, terminal_text_area};

/// Render the Chat panel: a CHAT | TERMINAL | GRAPH header bar over the
/// conversation, the terminal (its commands and output, then its prompt),
/// or the flow between the agent and the agents it spawned. While a record, file, process or
/// the run graph covers the body, the bar names it. BENCH shows a
/// benchmark, or the form that sets one up, under a bar with no tabs.
pub(super) fn render_chat_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Chat;
    let hosted = app.host_chrome();
    let panel = area;
    let (bar, area) = header::split(area, hosted);
    // A view over the body draws it focused; a member's chat is the chat.
    let named = detail_header(app, theme)
        .map(|named| (named, true))
        .or_else(|| member_chat_header(app, theme).map(|named| (named, is_focused)));
    if let Some(((title, info), focused)) = named {
        render_detail_bar(frame, app, (panel, bar), (&title, info), focused, theme);
    } else {
        render_tab_bar(frame, app, (panel, bar), is_focused, theme);
    }

    // An app showing nothing shows none of the agent's tabs either.
    let empty = app.app_empty();
    if !empty && render_other_tab(frame, area, app, theme, is_focused) {
        return;
    }

    let Some(body) = render_body_block(frame, area, (is_focused, hosted), theme) else {
        return;
    };

    let terminal = app.terminal_pane() && !empty;
    let padded = terminal_text_area(body, terminal);
    app.set_chat_width(padded.width);
    if app.terminal_inline() {
        terminal::render_terminal(frame, (area, body, padded), app, theme, is_focused);
        return;
    }
    app.set_chat_text_area(padded);

    if empty {
        clear_chat_rows(app, padded);
        render_empty(frame, padded, &app.app_empty_message(), theme);
        return;
    }

    let tail = if terminal {
        LiveTail::default()
    } else {
        live_tail(app, theme)
    };
    if render_waiting(frame, app, padded, (terminal, &tail), theme) {
        return;
    }

    let rows = render_chat_lines(frame, (body, padded), app, theme, &tail);
    render_chat_scroll(frame, app, (area, padded), (&rows, is_focused), theme);
}

/// The chat bar's expand button in `cell`.
fn expand_button(app: &App, cell: Option<Rect>) -> header::ExpandButton {
    header::ExpandButton {
        expanded: app.chat_expanded(),
        hovered: app.hover() == Some(HoverTarget::ExpandChat),
        cell,
    }
}

/// The chat panel's header over a channel member's own chat: the
/// member's name, its code, role and the channel's `#slug`, and the close
/// button that goes back to the channel's chat. `None` elsewhere.
///
/// ```text
/// │ Reviewer           RE36 · agent · #zode ✕ │
/// ```
fn member_chat_header(app: &App, theme: &Theme) -> Option<(String, Option<HeaderInfo>)> {
    let (name, about) = app.member_chat_title()?;
    let short = app
        .member_chat()
        .and_then(|member| member.code.clone())
        .unwrap_or_default();
    let info = HeaderInfo {
        text: about,
        short,
        color: theme.colors.muted,
    };
    Some((name, Some(info)))
}

/// The bar naming the record, file, process or graph covering the body,
/// or the member whose own chat is on screen; lit when `focused`.
fn render_detail_bar(
    frame: &mut Frame,
    app: &mut App,
    (panel, bar): (Rect, Rect),
    (title, info): (&str, Option<HeaderInfo>),
    focused: bool,
    theme: &Theme,
) {
    let hosted = app.host_chrome();
    let expand = Some(expand_button(app, header::expand_cell(bar, true)));
    if !hosted {
        app.set_detail_close_area(header::close_cell(bar));
        app.set_chat_expand_area(header::expand_cell(bar, true));
    }
    let header = PanelHeader::<()> {
        layout: TabLayout::untabbed(bar.width),
        active: None,
        hovered: None,
        title: Some(title),
        info,
        close: Some(app.hover() == Some(HoverTarget::CloseDetail)),
        expand,
        scope: None,
        icon_tab: None,
        focused,
        hosted,
    };
    header::show(frame, app, (Pane::Chat, panel, bar), &header, theme);
}

/// The CHAT | TERMINAL | GRAPH tab bar, titled with the app's name.
fn render_tab_bar(
    frame: &mut Frame,
    app: &mut App,
    (panel, bar): (Rect, Rect),
    is_focused: bool,
    theme: &Theme,
) {
    let hosted = app.host_chrome();
    let tabs = header::tabs_beside_expand(bar);
    let expand = Some(expand_button(app, header::expand_cell(bar, false)));
    if !hosted {
        app.set_chat_tabs_area(Rect { width: tabs, ..bar });
        app.set_chat_expand_area(header::expand_cell(bar, false));
    }
    let hovered = match app.hover() {
        Some(HoverTarget::ChatTab(tab)) => Some(tab),
        _ => None,
    };
    let app_title = app.app_chat_title();
    let (title, info) = match &app_title {
        Some((name, about)) => (
            Some(name.as_str()),
            Some(about)
                .filter(|about| !about.trim().is_empty())
                .map(|about| HeaderInfo {
                    text: about.clone(),
                    short: String::new(),
                    color: theme.colors.muted,
                }),
        ),
        None => (None, None),
    };
    let header = PanelHeader {
        layout: app.chat_tab_layout(tabs),
        active: Some(app.chat_tab()),
        hovered,
        title,
        info,
        close: None,
        expand,
        scope: None,
        icon_tab: None,
        focused: is_focused,
        hosted,
    };
    header::show(frame, app, (Pane::Chat, panel, bar), &header, theme);
}

/// Draw the benchmark, the flow or the run log when one of them takes the
/// body; returns whether one did.
fn render_other_tab(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    theme: &Theme,
    is_focused: bool,
) -> bool {
    if app.bench_draws_center() {
        super::benchmarks::render_benchmarks(frame, area, app, theme, is_focused);
        return true;
    }
    if matches!(app.chat_tab(), ChatTab::Graph | ChatTab::Time) {
        super::flow::render_flow(frame, area, app, theme, is_focused);
        return true;
    }
    if app.chat_tab() == ChatTab::Log {
        super::run_log::render_run_log(frame, area, app, theme, is_focused);
        return true;
    }
    false
}

/// Forget the conversation's images, copy buttons and rows while `padded`
/// shows none.
fn clear_chat_rows(app: &mut App, padded: Rect) {
    app.set_chat_images(Vec::new());
    app.set_chat_code_copies(Vec::new());
    app.clear_chat_links();
    app.set_chat_rows(Some((padded, 0)));
}

/// Draw the loading line while the agent's history is on its way, an
/// empty channel's welcome, or nothing while there is nothing to show;
/// returns whether it did.
fn render_waiting(
    frame: &mut Frame,
    app: &mut App,
    padded: Rect,
    (terminal, tail): (bool, &LiveTail),
    theme: &Theme,
) -> bool {
    if !terminal && app.chat_loading() {
        // The agent's history is on its way: neither the welcome text nor
        // a partial chat, which would flash before the real one.
        clear_chat_rows(app, padded);
        if app.chat_loading_shown() {
            render_chat_loading(frame, padded, theme);
        }
        return true;
    }
    if app.pane_is_empty() && tail.lines.is_empty() {
        clear_chat_rows(app, padded);
        if let Some(welcome) = app.channel_welcome() {
            super::channel_welcome::render_channel_welcome(frame, padded, &welcome, theme);
        }
        return true;
    }
    false
}

/// Draw the chat's scrollbar and the count of lines below, and hand the
/// pointer the scrollbar to drag.
fn render_chat_scroll(
    frame: &mut Frame,
    app: &mut App,
    (area, padded): (Rect, Rect),
    (rows, is_focused): (&ChatRows, bool),
    theme: &Theme,
) {
    let view = ScrollView {
        total: rows.total,
        visible: padded.height as usize,
        first: rows.first,
    };
    render_scrollbar(frame, area, padded, view, is_focused, theme);
    let last = view.total.saturating_sub(view.visible);
    let scrollbar = chat_scrollbar(ScrollTarget::Chat, (area, padded), view, last);
    app.set_chat_scrollbar(scrollbar, frame.buffer_mut());
    let below = app.scroll_offset().saturating_sub(rows.padding);
    render_lines_below_hint(frame, area, below, theme);
}

/// The scrollbar [`render_scrollbar`] draws on the chat panel `panel`
/// beside the text in `rows` for `view`, as the pointer drags it to
/// scroll `target` as far as its first row `last`.
pub(super) fn chat_scrollbar(
    target: ScrollTarget,
    (panel, rows): (Rect, Rect),
    view: ScrollView,
    last: usize,
) -> Option<ScrollbarArea> {
    scrollbar_track(panel, rows, view).map(|track| ScrollbarArea {
        target,
        track,
        total: view.total,
        visible: view.visible,
        first: view.first,
        last,
        open: track.height,
        margin: track.x.saturating_sub(rows.right()),
    })
}

/// `↓ 42 lines below` on the chat panel's bottom border while scrolled up.
fn render_lines_below_hint(frame: &mut Frame, panel: Rect, below: usize, theme: &Theme) {
    if below == 0 || panel.width < 4 || panel.height < 2 {
        return;
    }
    let label = format!(
        " ↓ {below} line{} below ",
        if below == 1 { "" } else { "s" }
    );
    let border_row = Rect {
        x: panel.x + 1,
        y: panel.y + panel.height - 1,
        width: panel.width - 2,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            label,
            Style::default().fg(theme.colors.muted),
        )))
        .alignment(ratatui::layout::Alignment::Right),
        border_row,
    );
}

/// What the chat says once it has waited a moment for its history.
const CHAT_LOADING: &str = "Loading chat…";

fn render_chat_loading(frame: &mut Frame, area: Rect, theme: &Theme) {
    let line = Line::from(Span::styled(
        CHAT_LOADING,
        Style::default().fg(theme.colors.faint),
    ));
    frame.render_widget(Paragraph::new(line), area);
}
