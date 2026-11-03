//! Chat lines for each message, kept between frames.

use super::activity::{activity_lines, group_header, GroupView, TAIL_STEPS};
use super::command::shell_lines;
use super::markdown::{parse_content_segments, ContentSegment};
use super::segments::{
    message_header, message_style, render_code_segment, render_text_segment, reply_meta,
};
use crate::app::{ToggleRow, ToggleTarget};
use crate::components::{Activity, Message};
use crate::Theme;
use ratatui::text::Line;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::ops::Range;

/// How the groups of agent steps in the chat are shown.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GroupState<'a> {
    /// Groups the user opened (`true`) or closed, by first message id
    pub open: &'a HashMap<u64, bool>,
    /// Whether groups the user left alone are open (`Ctrl+O`)
    pub expand_all: bool,
    /// Whether a turn is in flight, so the last group may still grow
    pub turn_live: bool,
}

/// The lines each chat block is drawn as, so a frame only formats the
/// blocks that are new or changed since the last one. A block is one
/// message, or a run of agent steps drawn as a group.
#[derive(Debug, Default)]
pub struct ChatLineCache {
    /// Text width the cached lines were wrapped to
    width: usize,
    /// Theme the cached lines were styled with
    theme: Option<Theme>,
    /// Lines per block, by the id of its first message
    entries: HashMap<u64, BlockLines>,
    /// Blocks in conversation order, as of the last update
    order: Vec<u64>,
}

/// One block of the chat as drawn.
#[derive(Debug)]
pub(crate) struct BlockLines {
    /// What the block was drawn from
    revision: u64,
    /// The block's lines
    pub lines: Vec<Line<'static>>,
    /// Rows that expand or collapse something, `line` counted from the
    /// block's first line
    pub toggles: Vec<ToggleRow>,
}

impl ChatLineCache {
    /// Bring the cache up to date with `messages` drawn `width` columns wide.
    /// Activity still in progress is redrawn every frame, with `spinner`.
    ///
    /// Returns `true` when every block was reflowed because the width or
    /// theme changed, so line positions from the last frame no longer apply.
    pub(crate) fn update(
        &mut self,
        messages: &VecDeque<Message>,
        width: usize,
        theme: &Theme,
        spinner: &str,
        groups: &GroupState<'_>,
    ) -> bool {
        let reflowed = self.width != width || self.theme.as_ref() != Some(theme);
        if reflowed {
            self.width = width;
            self.theme = Some(theme.clone());
            self.entries.clear();
        }
        let runs = block_runs(messages);
        self.order.clear();
        for (i, run) in runs.iter().enumerate() {
            let members: Vec<&Message> = messages.range(run.clone()).collect();
            let key = members[0].version().0;
            let is_group = members.len() > 1;
            let live = members.iter().any(|m| m.is_live_activity())
                || (is_group && groups.turn_live && i + 1 == runs.len());
            let view = is_group
                .then(|| group_view(groups.open.get(&key).copied(), groups.expand_all, live));

            let mut hasher = std::hash::DefaultHasher::new();
            for member in &members {
                member.version().hash(&mut hasher);
            }
            (view, live).hash(&mut hasher);
            let revision = hasher.finish();

            if live
                || self
                    .entries
                    .get(&key)
                    .is_none_or(|block| block.revision != revision)
            {
                let (lines, toggles) = match view {
                    Some(view) => group_lines(&members, view, live, width, theme, spinner),
                    None => single_lines(members[0], width, theme, spinner),
                };
                self.entries.insert(
                    key,
                    BlockLines {
                        revision,
                        lines,
                        toggles,
                    },
                );
            }
            self.order.push(key);
        }
        if self.entries.len() > self.order.len() {
            let live: HashSet<u64> = self.order.iter().copied().collect();
            self.entries.retain(|id, _| live.contains(id));
        }
        reflowed
    }

    /// Blocks in conversation order as of the last [`Self::update`]. A
    /// blank line goes between every two.
    pub(crate) fn blocks(&self) -> impl Iterator<Item = &BlockLines> {
        self.order.iter().filter_map(|key| self.entries.get(key))
    }
}

/// Messages grouped into blocks: consecutive agent steps share one.
fn block_runs(messages: &VecDeque<Message>) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    let is_activity = |i: usize| messages[i].activity_step().is_some();
    for i in 0..messages.len() {
        match runs.last_mut() {
            Some(run) if is_activity(i) && is_activity(run.end - 1) => run.end = i + 1,
            _ => runs.push(i..i + 1),
        }
    }
    runs
}

/// How much of a group to show: what the user chose, else everything under
/// `Ctrl+O`, else the latest steps while it runs.
fn group_view(open: Option<bool>, expand_all: bool, live: bool) -> GroupView {
    match open {
        Some(true) => GroupView::Full,
        Some(false) => GroupView::Collapsed,
        None if expand_all => GroupView::Full,
        None if live => GroupView::Tail,
        None => GroupView::Collapsed,
    }
}

/// A group's summary line, then the steps `view` shows.
fn group_lines(
    members: &[&Message],
    view: GroupView,
    live: bool,
    width: usize,
    theme: &Theme,
    spinner: &str,
) -> (Vec<Line<'static>>, Vec<ToggleRow>) {
    let steps: Vec<&Activity> = members.iter().filter_map(|m| m.activity_step()).collect();
    let mut lines = vec![group_header(&steps, view, live, spinner, width, theme)];
    let mut toggles = vec![ToggleRow {
        line: 0,
        target: ToggleTarget::Group(members[0].version().0),
        open: view != GroupView::Collapsed,
    }];
    let shown = match view {
        GroupView::Collapsed => 0,
        GroupView::Tail => TAIL_STEPS.min(members.len()),
        GroupView::Full => members.len(),
    };
    for member in &members[members.len() - shown..] {
        let Some(step) = member.activity_step() else {
            continue;
        };
        toggles.push(ToggleRow {
            line: lines.len(),
            target: ToggleTarget::Step(member.version().0),
            open: member.is_expanded(),
        });
        lines.extend(activity_lines(
            step,
            member.is_expanded(),
            true,
            spinner,
            width,
            theme,
        ));
    }
    (lines, toggles)
}

/// A block of one message; a lone agent step toggles on its header.
fn single_lines(
    message: &Message,
    width: usize,
    theme: &Theme,
    spinner: &str,
) -> (Vec<Line<'static>>, Vec<ToggleRow>) {
    let toggles = message
        .activity_step()
        .map(|_| ToggleRow {
            line: 0,
            target: ToggleTarget::Step(message.version().0),
            open: message.is_expanded(),
        })
        .into_iter()
        .collect();
    (message_lines(message, width, theme, spinner), toggles)
}

/// The header and body lines of one chat message.
pub(super) fn message_lines(
    message: &Message,
    width: usize,
    theme: &Theme,
    spinner: &str,
) -> Vec<Line<'static>> {
    if let Some(activity) = message.activity_step() {
        return activity_lines(
            activity,
            message.is_expanded(),
            false,
            spinner,
            width,
            theme,
        );
    }
    let timestamp = message.timestamp_local();
    if let Some(run) = message.shell_run() {
        return shell_lines(run, &timestamp, width, theme);
    }
    let (nick, nick_color, msg_color) = message_style(message.role(), theme);
    let meta = reply_meta(message.model(), message.usage());
    let mut lines = vec![message_header(
        nick, nick_color, &timestamp, &meta, width, theme,
    )];
    for segment in parse_content_segments(message.content()) {
        match segment {
            ContentSegment::Text(text) => {
                render_text_segment(&mut lines, &text, msg_color, width, message.role(), theme);
            }
            ContentSegment::CodeBlock { language, code } => {
                render_code_segment(&mut lines, &language, &code, width, theme);
            }
        }
    }
    lines
}
