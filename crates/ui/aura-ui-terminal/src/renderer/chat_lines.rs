//! Chat lines for each message, kept between frames.

mod stream;

use super::activity::{
    activity_lines, collapsed_hint, group_header, records_link, GroupView, StepNumber,
};
use super::command::shell_lines;
use super::diff::diff_message_lines;
use super::markdown::{parse_content_segments, ContentSegment};
use super::segments::{
    message_style, prefixed_lines, render_code_segment, render_text_segment, reply_footer,
    reply_meta,
};
use super::text::Measure;
use crate::app::{ImageHit, ToggleRow, ToggleTarget};
use crate::components::{Activity, Message, MessageRole};
use crate::Theme;
use ratatui::text::Line;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use stream::Settled;

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
#[derive(Debug, Clone, Copy)]
struct TurnPlace<'a> {
    /// The turn's latest reply, when the block is the turn's last from
    /// the agent. Its model, tokens, cost and who paid for them are drawn
    /// as one line after the block.
    meta: Option<&'a Message>,
    /// Number of the block's first step, counted from 1 across the turn
    first_step: usize,
    /// When the turn's prompt was sent, in Unix milliseconds
    started_ms: u64,
}

impl TurnPlace<'_> {
    /// A block outside any turn.
    const fn alone() -> Self {
        Self {
            meta: None,
            first_step: 1,
            started_ms: 0,
        }
    }
}

impl Hash for TurnPlace<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // A reply's version changes with what its footer is drawn from,
        // so the footer is only formatted when the block is drawn.
        self.meta.map(Message::version).hash(state);
        self.first_step.hash(state);
        self.started_ms.hash(state);
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
    /// Image thumbnails, `line` counted from the block's first line
    pub images: Vec<ImageHit>,
    /// Whether each line is prose, the user's or the agent's flowing text:
    /// `Some` of the columns its marker, quote bar or indent takes. Lines
    /// past the end are not.
    pub prose: Vec<Option<u16>>,
    /// How much of a reply that is still streaming is laid out for good
    settled: Option<Settled>,
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
                let mut settled = None;
                let (mut lines, mut toggles, mut prose) = match (shell, view) {
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
                                message_lines(member, (width, measure), theme, spinner);
                            prose.resize(lines.len(), None);
                            prose.extend(flags);
                            lines.extend(more);
                        }
                        (lines, Vec::new(), prose)
                    }
                    (None, None) if !self.terminal && stream::applies(members[0]) => {
                        let (mut lines, mut prose, mut state) = self
                            .entries
                            .remove(&key)
                            .map_or_else(Default::default, |block| {
                                (block.lines, block.prose, block.settled.unwrap_or_default())
                            });
                        state.extend(members[0].content(), (&mut lines, &mut prose), |text| {
                            content_lines(text, MessageRole::Assistant, (width, measure), theme)
                        });
                        settled = Some(state);
                        (lines, Vec::new(), prose)
                    }
                    (None, None) => {
                        single_lines(members[0], place, (width, measure), theme, spinner)
                    }
                };
                let mut images = Vec::new();
                if !self.terminal && members.len() == 1 && !members[0].images().is_empty() {
                    let (more, hits) = super::images::message_image_lines(members[0], width, theme);
                    let start = lines.len();
                    images.extend(hits.into_iter().map(|hit| ImageHit {
                        line: start + hit.line,
                        ..hit
                    }));
                    lines.extend(more);
                }
                if let Some(reply) = place.meta {
                    push_footer(&mut lines, reply, width, theme);
                }
                let user =
                    !self.terminal && members.len() == 1 && members[0].role() == MessageRole::User;
                if user {
                    frame_user_block(&mut lines, &mut toggles, &mut images, &mut prose);
                }
                self.entries.insert(
                    key,
                    BlockLines {
                        revision,
                        user,
                        lines,
                        toggles,
                        images,
                        prose,
                        settled,
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

/// The footer of `reply`, a turn's latest, under the `lines` of the
/// turn's last block from the agent: the model, tokens, cost and who paid
/// for them, on one line.
fn push_footer(lines: &mut Vec<Line<'static>>, reply: &Message, width: usize, theme: &Theme) {
    let meta = reply_meta(reply.model(), reply.usage(), reply.sources());
    let Some(footer) = reply_footer(&meta, width, theme) else {
        return;
    };
    // Separate the closing meta line from the reply body (or the
    // activity lines above it) with a blank row so it reads as a
    // distinct footer rather than another content line.
    let separated = lines
        .last()
        .is_none_or(|line| line.spans.iter().all(|s| s.content.trim().is_empty()));
    if !separated {
        lines.push(Line::from(""));
    }
    lines.push(footer);
}

/// Blank rows above and below a user's message: the frame's border, then
/// a row of padding between it and the text.
pub(crate) const USER_FRAME_ROWS: usize = 2;

/// Make room for the frame around a user's message: [`USER_FRAME_ROWS`]
/// blank rows before and after its lines, with the rows of its toggles,
/// images and prose moved down to match. The panel draws the border on
/// the outermost of them.
fn frame_user_block(
    lines: &mut Vec<Line<'static>>,
    toggles: &mut [ToggleRow],
    images: &mut [ImageHit],
    prose: &mut Vec<Option<u16>>,
) {
    let pad = USER_FRAME_ROWS;
    lines.splice(0..0, std::iter::repeat_n(Line::default(), pad));
    lines.extend(std::iter::repeat_n(Line::default(), pad));
    for toggle in toggles {
        toggle.line += pad;
    }
    for hit in images {
        hit.line += pad;
    }
    if !prose.is_empty() {
        prose.splice(0..0, std::iter::repeat_n(None, pad));
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

/// A one-line system message such as `policy: read_file allowed by …`.
fn is_notice(message: &Message) -> bool {
    message.role() == MessageRole::System
        && message.activity_step().is_none()
        && message.shell_run().is_none()
        && !message.content().trim().is_empty()
        && !message.content().contains('\n')
}

/// Each block's place in its turn. A prompt starts a turn; the agent's
/// last block in it carries the footer of the turn's latest reply, and
/// steps are numbered in order across the turn.
fn turn_places<'a>(shown: &[&'a Message], runs: &[Range<usize>]) -> Vec<TurnPlace<'a>> {
    let mut places: Vec<TurnPlace<'a>> = Vec::with_capacity(runs.len());
    let mut last_agent: Option<usize> = None;
    let mut meta: Option<&'a Message> = None;
    let mut steps = 0;
    let mut started_ms = shown.first().map_or(0, |m| m.timestamp_ms());
    for run in runs {
        let first = shown[run.start];
        if first.role() == MessageRole::User {
            close_turn(&mut places, last_agent.take(), meta.take());
            steps = 0;
            started_ms = first.timestamp_ms();
        }
        let agent = first.role() == MessageRole::Assistant;
        if agent {
            last_agent = Some(places.len());
        }
        places.push(TurnPlace {
            meta: None,
            first_step: steps + 1,
            started_ms,
        });
        steps += shown[run.clone()]
            .iter()
            .filter(|m| m.activity_step().is_some())
            .count();
        if agent && first.activity_step().is_none() && first.model().is_some() {
            meta = Some(first);
        }
    }
    close_turn(&mut places, last_agent, meta);
    places
}

/// Put the footer of a turn's latest reply, `meta`, under its last block
/// from the agent, at `last`.
fn close_turn<'a>(places: &mut [TurnPlace<'a>], last: Option<usize>, meta: Option<&'a Message>) {
    if let Some(place) = last.and_then(|at| places.get_mut(at)) {
        place.meta = meta;
    }
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
fn group_progress(steps: &[&Activity], place: &TurnPlace<'_>) -> String {
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
    place: &TurnPlace<'_>,
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
    let mut lines: Vec<Line<'static>> = Vec::new();
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
    let last = place.first_step + steps.len().saturating_sub(1);
    for member in members {
        let Some(step) = member.activity_step() else {
            continue;
        };
        let step_number = StepNumber::new(number, last);
        toggles.push(ToggleRow {
            line: lines.len(),
            target: ToggleTarget::Step(member.version().0),
            open: member.is_expanded(),
        });
        lines.extend(activity_lines(
            step,
            member.is_expanded(),
            Some(step_number),
            spinner,
            width,
            theme,
        ));
        push_records_link(
            &mut lines,
            &mut toggles,
            (step, member.is_expanded()),
            Some(step_number),
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
    number: Option<StepNumber>,
    place: &TurnPlace<'_>,
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
    place: &TurnPlace<'_>,
    (width, measure): (usize, Measure),
    theme: &Theme,
    spinner: &str,
) -> (Vec<Line<'static>>, Vec<ToggleRow>, Vec<Option<u16>>) {
    let Some(step) = message.activity_step() else {
        let (lines, prose) = message_lines(message, (width, measure), theme, spinner);
        return (lines, Vec::new(), prose);
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut toggles = vec![ToggleRow {
        line: 0,
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

/// The lines of one chat message, and whether each is prose. What the user
/// sent follows a `> ` prompt and system messages a `* `; the agent's
/// replies stand on their own, like a program's output.
fn message_lines(
    message: &Message,
    (width, measure): (usize, Measure),
    theme: &Theme,
    spinner: &str,
) -> (Vec<Line<'static>>, Vec<Option<u16>>) {
    if let Some(activity) = message.activity_step() {
        let lines = activity_lines(activity, message.is_expanded(), None, spinner, width, theme);
        return (lines, Vec::new());
    }
    if let Some(run) = message.shell_run() {
        let lines = shell_lines(run, &message.timestamp_local(), width, theme);
        return (lines, Vec::new());
    }
    if let Some(block) = message.diff_block() {
        return (diff_message_lines(block, width, theme), Vec::new());
    }
    let role = message.role();
    let (prompt, prompt_color, msg_color) = message_style(role, theme);
    if is_notice(message) {
        let lines = prefixed_lines(
            (prompt, prompt_color),
            message.content(),
            msg_color,
            width,
            Measure::GRID,
        );
        return (lines, Vec::new());
    }
    content_lines(message.content(), role, (width, measure), theme)
}

/// The lines of the text a message of `role` says, and whether each is
/// prose: fenced code set apart from the text around it.
fn content_lines(
    content: &str,
    role: MessageRole,
    (width, measure): (usize, Measure),
    theme: &Theme,
) -> (Vec<Line<'static>>, Vec<Option<u16>>) {
    let (prompt, prompt_color, msg_color) = message_style(role, theme);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut prose = Vec::new();
    for (i, segment) in parse_content_segments(content).into_iter().enumerate() {
        prose.resize(lines.len(), None);
        match segment {
            ContentSegment::Text(text) => {
                prose.extend(render_text_segment(
                    &mut lines,
                    &text,
                    (role, i == 0),
                    width,
                    theme,
                    measure,
                ));
            }
            ContentSegment::CodeBlock { language, code } => {
                if i == 0 && !prompt.is_empty() {
                    lines.extend(prefixed_lines(
                        (prompt, prompt_color),
                        "",
                        msg_color,
                        width,
                        Measure::GRID,
                    ));
                }
                render_code_segment(&mut lines, &language, &code, width, theme);
            }
        }
    }
    (lines, prose)
}
