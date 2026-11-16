//! Chat lines for each message, kept between frames.

use super::activity::{activity_lines, collapsed_hint, group_header, records_link, GroupView};
use super::command::shell_lines;
use super::diff::diff_message_lines;
use super::markdown::{parse_content_segments, ContentSegment};
use super::segments::{
    message_header, message_style, notice_lines, render_code_segment, render_text_segment,
    reply_meta,
};
use super::text::Measure;
use crate::app::{ToggleRow, ToggleTarget};
use crate::components::{Activity, Message, MessageRole};
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

/// Where a block falls in its turn: everything from one prompt to the next.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TurnPlace {
    /// Whether the block is the turn's first from the agent, drawn under
    /// the turn's `AURA` header
    header: bool,
    /// Model, tokens and cost of the turn's latest reply, for that header
    meta: Vec<String>,
    /// Number of the block's first step, counted from 1 across the turn
    first_step: usize,
    /// When the turn's prompt was sent, in Unix milliseconds
    started_ms: u64,
}

impl TurnPlace {
    /// A block outside any turn, drawn with its own header.
    const fn alone() -> Self {
        Self {
            header: true,
            meta: Vec::new(),
            first_step: 1,
            started_ms: 0,
        }
    }
}

/// The lines each chat block is drawn as, so a frame only formats the
/// blocks that are new or changed since the last one. A block is one
/// message, or a run of agent steps drawn as a group.
#[derive(Debug, Default)]
pub struct ChatLineCache {
    /// Whether this draws the terminal pane (only shell commands) rather
    /// than the chat (everything else)
    terminal: bool,
    /// Shell prompt the terminal pane's commands are drawn after
    prompt: String,
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
    /// Whether the block is a message the user sent
    pub user: bool,
    /// The block's lines
    pub lines: Vec<Line<'static>>,
    /// Rows that expand or collapse something, `line` counted from the
    /// block's first line
    pub toggles: Vec<ToggleRow>,
    /// Whether each line is prose, the user's or the agent's flowing text:
    /// `Some` of the columns its marker, quote bar or indent takes. Lines
    /// past the end are not.
    pub prose: Vec<Option<u16>>,
}

impl ChatLineCache {
    /// A cache for the terminal pane, which draws only shell commands.
    pub(crate) fn terminal() -> Self {
        Self {
            terminal: true,
            ..Self::default()
        }
    }

    /// Draw commands after `prompt`, redrawing them all if it changed.
    pub(crate) fn set_prompt(&mut self, prompt: &str) {
        if self.prompt != prompt {
            prompt.clone_into(&mut self.prompt);
            self.entries.clear();
        }
    }

    /// Blank rows between two blocks: chat messages are spaced apart, the
    /// terminal's commands follow each other like a shell's.
    pub(crate) const fn block_gap(&self) -> usize {
        if self.terminal {
            0
        } else {
            1
        }
    }

    /// Bring the cache up to date with the messages of its pane drawn
    /// `width` columns wide, prose as wide as `measure` makes it. Activity
    /// still in progress is redrawn every frame, with `spinner`.
    ///
    /// Returns `true` when every block was reflowed because the width or
    /// theme changed, so line positions from the last frame no longer apply.
    pub(crate) fn update(
        &mut self,
        messages: &VecDeque<Message>,
        (width, measure): (usize, Measure),
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
        let shown: Vec<&Message> = messages
            .iter()
            .filter(|m| m.shell_run().is_some() == self.terminal && !m.is_empty_reply())
            .collect();
        let runs = block_runs(&shown);
        let places = if self.terminal {
            vec![TurnPlace::alone(); runs.len()]
        } else {
            turn_places(&shown, &runs)
        };
        self.order.clear();
        for (i, (run, place)) in runs.iter().zip(&places).enumerate() {
            let members = &shown[run.clone()];
            let key = members[0].version().0;
            let is_group = members.len() > 1 && members[0].activity_step().is_some();
            let live = members.iter().any(|m| m.is_live_activity())
                || (is_group && groups.turn_live && i + 1 == runs.len());
            let view = is_group
                .then(|| group_view(groups.open.get(&key).copied(), groups.expand_all, live));

            let mut hasher = std::hash::DefaultHasher::new();
            for member in members {
                member.version().hash(&mut hasher);
            }
            (view, live, place).hash(&mut hasher);
            let revision = hasher.finish();

            if live
                || self
                    .entries
                    .get(&key)
                    .is_none_or(|block| block.revision != revision)
            {
                let shell = members[0].shell_run().filter(|_| self.terminal);
                let (lines, toggles, prose) = match (shell, view) {
                    (Some(run), _) => (
                        shell_lines(run, &self.prompt, width, theme),
                        Vec::new(),
                        Vec::new(),
                    ),
                    (None, Some(view)) => {
                        let (lines, toggles) =
                            group_lines(members, view, live, place, width, theme, spinner);
                        (lines, toggles, Vec::new())
                    }
                    (None, None) if members.len() > 1 => {
                        let mut lines = Vec::new();
                        let mut prose = Vec::new();
                        for member in members {
                            let (more, flags) =
                                message_lines(member, place, (width, measure), theme, spinner);
                            prose.resize(lines.len(), None);
                            prose.extend(flags);
                            lines.extend(more);
                        }
                        (lines, Vec::new(), prose)
                    }
                    (None, None) => {
                        single_lines(members[0], place, (width, measure), theme, spinner)
                    }
                };
                self.entries.insert(
                    key,
                    BlockLines {
                        revision,
                        user: !self.terminal
                            && members.len() == 1
                            && members[0].role() == MessageRole::User,
                        lines,
                        toggles,
                        prose,
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

/// Messages grouped into blocks: consecutive agent steps share one, and so
/// do consecutive notices.
fn block_runs(messages: &[&Message]) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    let is_activity = |i: usize| messages[i].activity_step().is_some();
    let is_notice = |i: usize| is_notice(messages[i]);
    for i in 0..messages.len() {
        match runs.last_mut() {
            Some(run)
                if (is_activity(i) && is_activity(run.end - 1))
                    || (is_notice(i) && is_notice(run.end - 1)) =>
            {
                run.end = i + 1;
            }
            _ => runs.push(i..i + 1),
        }
    }
    runs
}

/// A one-line system message such as `policy: read_file allowed by …`,
/// drawn on the same line as its time.
fn is_notice(message: &Message) -> bool {
    message.role() == MessageRole::System
        && message.activity_step().is_none()
        && message.shell_run().is_none()
        && !message.content().trim().is_empty()
        && !message.content().contains('\n')
}

/// Each block's place in its turn. A prompt starts a turn; the agent's
/// first block after it carries the turn's header, and steps are numbered
/// in order across the turn.
fn turn_places(shown: &[&Message], runs: &[Range<usize>]) -> Vec<TurnPlace> {
    let mut places: Vec<TurnPlace> = Vec::with_capacity(runs.len());
    let mut header_at: Option<usize> = None;
    let mut steps = 0;
    let mut started_ms = shown.first().map_or(0, |m| m.timestamp_ms());
    for run in runs {
        let first = shown[run.start];
        if first.role() == MessageRole::User {
            header_at = None;
            steps = 0;
            started_ms = first.timestamp_ms();
        }
        let agent = first.role() == MessageRole::Assistant;
        let header = agent && header_at.is_none();
        if header {
            header_at = Some(places.len());
        }
        places.push(TurnPlace {
            header,
            meta: Vec::new(),
            first_step: steps + 1,
            started_ms,
        });
        steps += shown[run.clone()]
            .iter()
            .filter(|m| m.activity_step().is_some())
            .count();
        if agent && first.activity_step().is_none() && first.model().is_some() {
            if let Some(at) = header_at {
                places[at].meta = reply_meta(first.model(), first.usage());
            }
        }
    }
    places
}

/// The `AURA` line over the agent's first block of a turn.
fn turn_header(
    message: &Message,
    place: &TurnPlace,
    width: usize,
    theme: &Theme,
) -> Option<Line<'static>> {
    if message.role() != MessageRole::Assistant || !place.header {
        return None;
    }
    let (nick, nick_color, _) = message_style(message.role(), theme);
    let own;
    let meta = if place.meta.is_empty() {
        own = reply_meta(message.model(), message.usage());
        &own
    } else {
        &place.meta
    };
    Some(message_header(
        nick,
        nick_color,
        &message.timestamp_local(),
        meta,
        width,
        theme,
    ))
}

/// How much of a group to show: what the user chose, else everything under
/// `Ctrl+O` or while it runs.
fn group_view(open: Option<bool>, expand_all: bool, live: bool) -> GroupView {
    match open {
        Some(true) => GroupView::Full,
        Some(false) => GroupView::Collapsed,
        None if expand_all || live => GroupView::Full,
        None => GroupView::Collapsed,
    }
}

/// ` · step 7 · 42s` after a running group's summary: the number of its
/// latest step in the turn, and how long the group has been going.
fn group_progress(steps: &[&Activity], place: &TurnPlace) -> String {
    let mut progress = format!(" · step {}", place.first_step + steps.len() - 1);
    if let Some(elapsed) = steps
        .first()
        .and_then(|step| step.started())
        .map(|started| started.elapsed())
        .filter(|elapsed| elapsed.as_secs() > 0)
    {
        progress.push_str(" · ");
        progress.push_str(&super::panels::format_elapsed(elapsed));
    }
    progress
}

/// A group's summary line, then every step, numbered, or a row to show
/// them while the group is collapsed.
fn group_lines(
    members: &[&Message],
    view: GroupView,
    live: bool,
    place: &TurnPlace,
    width: usize,
    theme: &Theme,
    spinner: &str,
) -> (Vec<Line<'static>>, Vec<ToggleRow>) {
    let steps: Vec<&Activity> = members.iter().filter_map(|m| m.activity_step()).collect();
    let progress = if live {
        group_progress(&steps, place)
    } else {
        String::new()
    };
    let mut lines: Vec<Line<'static>> = turn_header(members[0], place, width, theme)
        .into_iter()
        .collect();
    let group = ToggleTarget::Group(members[0].version().0);
    let mut toggles = vec![ToggleRow {
        line: lines.len(),
        target: group,
        open: view != GroupView::Collapsed,
    }];
    lines.push(group_header(
        &steps, view, live, &progress, spinner, width, theme,
    ));
    if view == GroupView::Collapsed {
        toggles.push(ToggleRow {
            line: lines.len(),
            target: group,
            open: false,
        });
        lines.push(collapsed_hint(steps.len(), theme));
        return (lines, toggles);
    }
    let mut number = place.first_step;
    for member in members {
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
            Some(number),
            spinner,
            width,
            theme,
        ));
        push_records_link(
            &mut lines,
            &mut toggles,
            (step, member.is_expanded()),
            Some(number),
            place,
            (width, theme),
        );
        number += 1;
    }
    (lines, toggles)
}

/// After an open thinking step, the row that jumps to the records its
/// turn wrote.
fn push_records_link(
    lines: &mut Vec<Line<'static>>,
    toggles: &mut Vec<ToggleRow>,
    (step, expanded): (&Activity, bool),
    number: Option<usize>,
    place: &TurnPlace,
    (width, theme): (usize, &Theme),
) {
    if !expanded || !matches!(step, Activity::Thinking(_)) {
        return;
    }
    toggles.push(ToggleRow {
        line: lines.len(),
        target: ToggleTarget::TurnRecords(place.started_ms),
        open: false,
    });
    lines.push(records_link(number, width, theme));
}

/// A block of one message; a lone agent step toggles on its header.
fn single_lines(
    message: &Message,
    place: &TurnPlace,
    (width, measure): (usize, Measure),
    theme: &Theme,
    spinner: &str,
) -> (Vec<Line<'static>>, Vec<ToggleRow>, Vec<Option<u16>>) {
    let Some(step) = message.activity_step() else {
        let (lines, prose) = message_lines(message, place, (width, measure), theme, spinner);
        return (lines, Vec::new(), prose);
    };
    let mut lines: Vec<Line<'static>> = turn_header(message, place, width, theme)
        .into_iter()
        .collect();
    let mut toggles = vec![ToggleRow {
        line: lines.len(),
        target: ToggleTarget::Step(message.version().0),
        open: message.is_expanded(),
    }];
    lines.extend(activity_lines(
        step,
        message.is_expanded(),
        None,
        spinner,
        width,
        theme,
    ));
    push_records_link(
        &mut lines,
        &mut toggles,
        (step, message.is_expanded()),
        None,
        place,
        (width, theme),
    );
    (lines, toggles, Vec::new())
}

/// The header and body lines of one chat message, and whether each is
/// prose. The agent's replies after the first block of a turn go without
/// a header, under the turn's.
fn message_lines(
    message: &Message,
    place: &TurnPlace,
    (width, measure): (usize, Measure),
    theme: &Theme,
    spinner: &str,
) -> (Vec<Line<'static>>, Vec<Option<u16>>) {
    if let Some(activity) = message.activity_step() {
        let lines = activity_lines(activity, message.is_expanded(), None, spinner, width, theme);
        return (lines, Vec::new());
    }
    let timestamp = message.timestamp_local();
    if let Some(run) = message.shell_run() {
        return (shell_lines(run, &timestamp, width, theme), Vec::new());
    }
    if let Some(block) = message.diff_block() {
        return (
            diff_message_lines(block, &timestamp, width, theme),
            Vec::new(),
        );
    }
    let (nick, nick_color, msg_color) = message_style(message.role(), theme);
    if is_notice(message) {
        let lines = notice_lines(
            nick,
            nick_color,
            &timestamp,
            message.content(),
            msg_color,
            width,
            theme,
        );
        return (lines, Vec::new());
    }
    let mut lines: Vec<Line<'static>> = if message.role() == MessageRole::Assistant {
        turn_header(message, place, width, theme)
            .into_iter()
            .collect()
    } else {
        let meta = reply_meta(message.model(), message.usage());
        vec![message_header(
            nick, nick_color, &timestamp, &meta, width, theme,
        )]
    };
    let mut prose = Vec::new();
    for segment in parse_content_segments(message.content()) {
        prose.resize(lines.len(), None);
        match segment {
            ContentSegment::Text(text) => {
                let role = message.role();
                prose.extend(render_text_segment(
                    &mut lines, &text, msg_color, width, role, theme, measure,
                ));
            }
            ContentSegment::CodeBlock { language, code } => {
                render_code_segment(&mut lines, &language, &code, width, theme);
            }
        }
    }
    (lines, prose)
}
