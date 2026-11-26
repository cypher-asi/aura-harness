//! Chat lines for each message, kept between frames.

mod stream;

use super::activity::{
    activity_lines, collapsed_hint, group_header_of, records_link, running_tool_lines, GroupView,
    Shown, StepNumber, StepTally, Tally, ToolParts,
};
use super::boot::{boot_copy, boot_lines};
use super::command::shell_lines;
use super::diff::diff_message_lines;
use super::markdown::{parse_content_segments, ContentSegment};
use super::segments::{
    message_style, prefixed_lines, render_code_segment, render_text_segment, reply_footer,
    reply_meta,
};
use super::text::Measure;
use super::MoreRows;
use crate::app::{CodeCopyHit, ImageHit, ToggleRow, ToggleTarget};
use crate::components::{Activity, Message, MessageRole, MoreSection};
use crate::events::{
    merge_timeline, ChannelPostAuthor, ChannelPostView, MessageData, TimelineItem,
};
use crate::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::OnceLock;
use stream::{Laid, Settled};

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
    /// Copy buttons of code blocks, `line` counted from the block's first
    /// line
    pub codes: Vec<CodeCopyHit>,
    /// Whether each line is prose, the user's or the agent's flowing text:
    /// `Some` of the columns its marker, quote bar or indent takes. Lines
    /// past the end are not.
    pub prose: Vec<Option<u16>>,
    /// How much of a reply that is still streaming is laid out for good
    settled: Option<Settled>,
    /// The steps of a group still running, by the id of each step's
    /// message, so its next frame lays out only the steps that changed
    steps: HashMap<u64, StepLines>,
}

/// One step of a live group as drawn. A group grows while its turn runs,
/// and is laid out again every frame for its spinner and clocks; a step
/// that is done looks the same from frame to frame, so its lines and its
/// part of the group's summary (which parse the call's arguments, up to
/// hundreds of KB for a write) are kept.
#[derive(Debug)]
struct StepLines {
    /// The step's message version, which every change to it moves
    version: (u64, u64),
    /// What the step adds to the group's summary line
    tally: StepTally,
    /// The step's lines, its rows that show more and the number they were
    /// drawn with, or `None` while it runs, as its spinner and clock
    /// change every frame
    lines: Option<(Option<StepNumber>, Vec<Line<'static>>, MoreRows)>,
    /// What a running tool step is drawn from besides its header, and the
    /// number it was drawn with
    running: Option<(Option<StepNumber>, ToolParts)>,
}

impl StepLines {
    fn new(member: &Message, step: &Activity) -> Self {
        Self {
            version: member.version(),
            tally: StepTally::of(step),
            lines: None,
            running: None,
        }
    }

    /// What `kept` had of `member` when it has not changed since, else a
    /// fresh start.
    fn take(kept: Option<&mut KeptSteps>, member: &Message, step: &Activity) -> Self {
        kept.and_then(|kept| kept.remove(&member.version().0))
            .filter(|before| before.version == member.version())
            .unwrap_or_else(|| Self::new(member, step))
    }

    /// The lines of `step`, `member`'s, numbered `number`: the ones kept
    /// when it was drawn the same way before, else laid out afresh and
    /// kept when `keeping`. A finished step keeps its lines; a running
    /// tool keeps all but its header, which turns `spinner` and counts
    /// the seconds.
    fn draw(
        &mut self,
        (member, step): (&Message, &Activity),
        number: Option<StepNumber>,
        keeping: bool,
        (width, theme, spinner): (usize, &Theme, &str),
    ) -> (Vec<Line<'static>>, MoreRows) {
        if let Some((drawn_as, lines, more)) = &self.lines {
            if *drawn_as == number {
                return (lines.clone(), more.clone());
            }
        }
        let shown = Shown::of(member);
        let (lines, more) = match step {
            Activity::Tool(tool) if keeping && step.is_running() => {
                let parts = match self.running.take() {
                    Some((drawn_as, parts)) if drawn_as == number => parts,
                    _ => ToolParts::of(tool, shown, number, width, theme),
                };
                let drawn = running_tool_lines(tool, &parts, number, spinner, width, theme);
                self.running = Some((number, parts));
                drawn
            }
            _ => activity_lines(step, shown, number, spinner, width, theme),
        };
        if keeping && !step.is_running() {
            self.lines = Some((number, lines.clone(), more.clone()));
        }
        (lines, more)
    }
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
    /// still in progress is redrawn every frame, with `spinner`. The chat
    /// (not the terminal pane) shows `posts`, the posts of the channel the
    /// agent coordinates, among its messages by time.
    ///
    /// Returns `true` when every block was reflowed because the width or
    /// theme changed, so line positions from the last frame no longer apply.
    pub(crate) fn update(
        &mut self,
        (messages, posts): (&VecDeque<Message>, &[ChannelPostView]),
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
        let history: Vec<&Message> = messages
            .iter()
            .filter(|m| m.shell_run().is_some() == self.terminal && !m.is_empty_reply())
            .collect();
        let posts: &[ChannelPostView] = if self.terminal { &[] } else { posts };
        let post_messages: Vec<Message> = posts.iter().map(post_message).collect();
        let shown = interleave_posts(history, posts, &post_messages);
        let runs = block_runs(&shown);
        let places = if self.terminal {
            vec![TurnPlace::alone(); runs.len()]
        } else {
            turn_places(&shown, &runs)
        };
        let layout = Layout {
            terminal: self.terminal,
            prompt: &self.prompt,
            width,
            measure,
            theme,
            spinner,
        };
        let mut stale = Vec::new();
        self.order.clear();
        for (i, (run, place)) in runs.iter().zip(&places).enumerate() {
            let members = &shown[run.clone()];
            let key = members[0].version().0;
            let is_group = members.len() > 1 && members[0].activity_step().is_some();
            let live = members
                .iter()
                .any(|m| m.is_live_activity() || m.is_live_boot())
                || (is_group && groups.turn_live && i + 1 == runs.len());
            let view = is_group
                .then(|| group_view(groups.open.get(&key).copied(), groups.expand_all, live));

            let mut hasher = std::hash::DefaultHasher::new();
            for member in members {
                member.version().hash(&mut hasher);
            }
            (view, live, place).hash(&mut hasher);
            let revision = hasher.finish();

            self.order.push(key);
            if !live
                && self
                    .entries
                    .get(&key)
                    .is_some_and(|block| block.revision == revision)
            {
                continue;
            }
            let block = Stale {
                key,
                revision,
                members,
                view,
                live,
                place: *place,
            };
            if layout.streams(&block) || layout.keeps_steps(&block) {
                let before = self.entries.remove(&key);
                self.entries.insert(key, layout.block(&block, before));
            } else {
                stale.push(block);
            }
        }
        for (block, lines) in stale.iter().zip(layout.blocks(&stale)) {
            self.entries.insert(block.key, lines);
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

/// Blocks to lay out at once from which they are laid out across
/// threads: fewer take less time than handing them out does.
const PARALLEL_BLOCKS: usize = 16;

/// Most threads blocks are laid out on.
const MOST_LAYOUT_THREADS: usize = 8;

/// What every block of a frame is laid out with.
#[derive(Clone, Copy)]
struct Layout<'a> {
    terminal: bool,
    prompt: &'a str,
    width: usize,
    measure: Measure,
    theme: &'a Theme,
    spinner: &'a str,
}

/// A block whose lines are out of date.
struct Stale<'a> {
    /// The id of its first message
    key: u64,
    revision: u64,
    members: &'a [&'a Message],
    view: Option<GroupView>,
    live: bool,
    place: TurnPlace<'a>,
}

impl Layout<'_> {
    /// Whether `block` is a reply still being written, which is laid out
    /// from how it was the last time.
    fn streams(self, block: &Stale<'_>) -> bool {
        !self.terminal
            && block.view.is_none()
            && block.members.len() == 1
            && stream::applies(block.members[0])
    }

    /// Whether `block` is a group of steps, or one step, still running,
    /// laid out from the steps it had the last time.
    fn keeps_steps(self, block: &Stale<'_>) -> bool {
        block.live
            && (block.view.is_some()
                || (block.members.len() == 1 && block.members[0].activity_step().is_some()))
    }

    /// The lines of `blocks`, in order. A width or theme change, an agent
    /// switch or a chat's first frame lays out every block, which is
    /// shared out across threads.
    fn blocks(self, blocks: &[Stale<'_>]) -> Vec<BlockLines> {
        if blocks.len() >= PARALLEL_BLOCKS {
            if let Some(pool) = layout_threads() {
                return pool.install(|| {
                    blocks
                        .par_iter()
                        .map(|block| self.block(block, None))
                        .collect()
                });
            }
        }
        blocks.iter().map(|block| self.block(block, None)).collect()
    }

    /// The lines of `block`. A reply still being written goes on from
    /// `before`, how it was laid out the last time.
    fn block(self, block: &Stale<'_>, before: Option<BlockLines>) -> BlockLines {
        let Self {
            terminal,
            width,
            measure,
            theme,
            spinner,
            ..
        } = self;
        let members = block.members;
        let shell = members[0].shell_run().filter(|_| terminal);
        let mut settled = None;
        let mut steps = HashMap::new();
        let ((mut lines, mut prose, mut codes), mut toggles) = match (shell, block.view) {
            (Some(run), _) => (
                (
                    shell_lines(run, self.prompt, width, theme),
                    Vec::new(),
                    Vec::new(),
                ),
                Vec::new(),
            ),
            (None, Some(view)) => {
                // Only a live group is laid out again while it stays the
                // same, so only its steps are worth keeping.
                let kept = block
                    .live
                    .then(|| before.map(|block| block.steps).unwrap_or_default());
                let (lines, toggles, kept) = group_lines(
                    members,
                    (view, block.live),
                    &block.place,
                    (width, theme, spinner),
                    kept,
                );
                steps = kept;
                ((lines, Vec::new(), Vec::new()), toggles)
            }
            (None, None) if members.len() > 1 => {
                let mut laid = Laid::default();
                let mut toggles = Vec::new();
                for member in members {
                    let (more, rows) = message_lines(member, (width, measure), theme, spinner);
                    toggles.extend(more_toggles(member, rows, laid.0.len()));
                    stream::add((&mut laid.0, &mut laid.1, &mut laid.2), more);
                }
                (laid, toggles)
            }
            (None, None) if self.streams(block) => {
                let (mut lines, mut prose, mut codes, mut state) =
                    before.map_or_else(Default::default, |block| {
                        (
                            block.lines,
                            block.prose,
                            block.codes,
                            block.settled.unwrap_or_default(),
                        )
                    });
                state.extend(
                    members[0].content(),
                    (&mut lines, &mut prose, &mut codes),
                    |text| content_lines(text, MessageRole::Assistant, (width, measure), theme),
                );
                settled = Some(state);
                ((lines, prose, codes), Vec::new())
            }
            (None, None) => {
                // A lone step that runs is drawn again every frame; one
                // that is done is cached whole, so keeps nothing.
                let mut kept = (block.live && members[0].activity_step().is_some())
                    .then(|| before.map(|block| block.steps).unwrap_or_default());
                let laid = single_lines(
                    members[0],
                    &block.place,
                    (width, measure),
                    theme,
                    spinner,
                    kept.as_mut(),
                );
                steps = kept.unwrap_or_default();
                laid
            }
        };
        if let Some(author) = members[0].author().filter(|_| members.len() == 1) {
            put_author_label(author, (&mut lines, &mut toggles, &mut prose), theme);
        }
        let mut images = Vec::new();
        if !terminal && members.len() == 1 && !members[0].images().is_empty() {
            let (more, hits) = super::images::message_image_lines(members[0], width, theme);
            let start = lines.len();
            images.extend(hits.into_iter().map(|hit| ImageHit {
                line: start + hit.line,
                ..hit
            }));
            lines.extend(more);
        }
        if let Some(reply) = block.place.meta {
            push_footer(&mut lines, reply, width, theme);
        }
        let user = !terminal && members.len() == 1 && members[0].role() == MessageRole::User;
        if user {
            frame_user_block(
                &mut lines,
                &mut toggles,
                (&mut images, &mut codes),
                &mut prose,
            );
        }
        BlockLines {
            revision: block.revision,
            user,
            lines,
            toggles,
            images,
            codes,
            prose,
            settled,
            steps,
        }
    }
}

/// The threads chat blocks are laid out on, or `None` with one core.
/// They start with the first chat long enough to need them, from the
/// thread that draws the UI, so they run at its priority and not the
/// agents'.
fn layout_threads() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism()
            .map_or(1, NonZeroUsize::get)
            .min(MOST_LAYOUT_THREADS);
        if threads < 2 {
            return None;
        }
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|index| format!("aura-layout-{index}"))
            .build()
            .map_err(|error| tracing::warn!(%error, "laying chats out on one thread"))
            .ok()
    })
    .as_ref()
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
/// images, copy buttons and prose moved down to match. The panel draws
/// the border on the outermost of them.
fn frame_user_block(
    lines: &mut Vec<Line<'static>>,
    toggles: &mut [ToggleRow],
    (images, codes): (&mut [ImageHit], &mut [CodeCopyHit]),
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
    for hit in codes {
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
        && message.boot_console().is_none()
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
        // A post neither starts the agent's turn nor ends it.
        if first.author().is_some() {
            places.push(TurnPlace {
                meta: None,
                first_step: steps + 1,
                started_ms,
            });
            continue;
        }
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

/// Set on the id of a channel post's message, so it never meets the ids
/// of the conversation's messages, which count up from 0.
const POST_ID: u64 = 1 << 63;

/// The message a channel post is drawn as: the user's as a prompt, an
/// agent's as a reply, each under its author's label. Its id comes from
/// the post's `seq` and its revision from what it shows, so the lines laid
/// out for it are reused until it changes.
fn post_message(post: &ChannelPostView) -> Message {
    let (role, author) = match &post.author {
        ChannelPostAuthor::User => (MessageRole::User, "You"),
        ChannelPostAuthor::Agent { code, name, .. } => (
            MessageRole::Assistant,
            code.as_deref()
                .filter(|code| !code.is_empty())
                .unwrap_or(name),
        ),
    };
    let label = if post.reply_to.is_some() {
        format!("{author} ↩")
    } else {
        author.to_string()
    };
    let mut hasher = std::hash::DefaultHasher::new();
    (&label, &post.text, post.at_ms).hash(&mut hasher);
    let at = i64::try_from(post.at_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .unwrap_or_default();
    Message::post(
        (POST_ID | post.seq, hasher.finish()),
        role,
        label,
        (&post.text, at),
    )
}

/// `history` with `post_messages`, the messages of `posts`, among it in
/// the order [`merge_timeline`] puts them.
fn interleave_posts<'a>(
    history: Vec<&'a Message>,
    posts: &[ChannelPostView],
    post_messages: &'a [Message],
) -> Vec<&'a Message> {
    if posts.is_empty() {
        return history;
    }
    // Only the times are merged on; each list keeps its order, so the
    // merged entries are taken from the front of their own list.
    let stamps: Vec<MessageData> = history
        .iter()
        .map(|message| MessageData {
            timestamp_ms: Some(message.timestamp_ms()),
            ..MessageData::new(message.role(), String::new())
        })
        .collect();
    let mut history = history.into_iter();
    let mut post_messages = post_messages.iter();
    merge_timeline(&stamps, posts)
        .into_iter()
        .filter_map(|item| match item {
            TimelineItem::Message(_) => history.next(),
            TimelineItem::Post(_) => post_messages.next(),
        })
        .collect()
}

/// Put a post's `author` label above its lines, moving the rows of its
/// toggles and prose down to match.
fn put_author_label(
    author: &str,
    (lines, toggles, prose): (
        &mut Vec<Line<'static>>,
        &mut [ToggleRow],
        &mut Vec<Option<u16>>,
    ),
    theme: &Theme,
) {
    let style = Style::default()
        .fg(theme.colors.primary)
        .add_modifier(Modifier::BOLD);
    lines.insert(0, Line::from(Span::styled(author.to_string(), style)));
    for toggle in toggles {
        toggle.line += 1;
    }
    if !prose.is_empty() {
        prose.insert(0, None);
    }
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

/// What a live group keeps of its steps from frame to frame, by the id of
/// each step's message.
type KeptSteps = HashMap<u64, StepLines>;

/// A group's summary line, then every step, numbered, or a row to show
/// them while the group is collapsed.
///
/// A live group passes what it `kept` of its steps the last frame: a step
/// whose message has not changed since is drawn from that instead of
/// being laid out again (see [`StepLines::draw`]), and what the steps are
/// drawn as now is returned for the next frame.
fn group_lines(
    members: &[&Message],
    (view, live): (GroupView, bool),
    place: &TurnPlace<'_>,
    (width, theme, spinner): (usize, &Theme, &str),
    mut kept: Option<KeptSteps>,
) -> (Vec<Line<'static>>, Vec<ToggleRow>, KeptSteps) {
    let steps: Vec<(&Message, &Activity)> = members
        .iter()
        .filter_map(|member| member.activity_step().map(|step| (*member, step)))
        .collect();
    let keeping = kept.is_some();
    let mut drawn: Vec<StepLines> = steps
        .iter()
        .map(|&(member, step)| StepLines::take(kept.as_mut(), member, step))
        .collect();
    let progress = if live {
        let activities: Vec<&Activity> = steps.iter().map(|&(_, step)| step).collect();
        group_progress(&activities, place)
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
    let tally = Tally::of(drawn.iter().map(|step| &step.tally));
    lines.push(group_header_of(
        &tally, view, live, &progress, spinner, width, theme,
    ));
    if view == GroupView::Collapsed {
        toggles.push(ToggleRow {
            line: lines.len(),
            target: group,
            open: false,
        });
        lines.push(collapsed_hint(steps.len(), theme));
    } else {
        let mut number = place.first_step;
        let last = place.first_step + steps.len().saturating_sub(1);
        for (&(member, step), kept) in steps.iter().zip(&mut drawn) {
            let step_number = StepNumber::new(number, last);
            toggles.push(ToggleRow {
                line: lines.len(),
                target: ToggleTarget::Step(member.version().0),
                open: member.is_expanded(),
            });
            let (step_lines, more) = kept.draw(
                (member, step),
                Some(step_number),
                keeping,
                (width, theme, spinner),
            );
            toggles.extend(more_toggles(member, more, lines.len()));
            lines.extend(step_lines);
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
    }
    let kept = if keeping {
        steps
            .iter()
            .zip(drawn)
            .map(|(&(member, _), step)| (member.version().0, step))
            .collect()
    } else {
        KeptSteps::new()
    };
    (lines, toggles, kept)
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

/// A block of one message, and its rows that toggle; a lone agent step
/// toggles on its header. A lone step still running is drawn from what
/// it `kept` the last frame, as a live group's steps are.
fn single_lines(
    message: &Message,
    place: &TurnPlace<'_>,
    (width, measure): (usize, Measure),
    theme: &Theme,
    spinner: &str,
    kept: Option<&mut KeptSteps>,
) -> (Laid, Vec<ToggleRow>) {
    let Some(step) = message.activity_step() else {
        let (laid, more) = message_lines(message, (width, measure), theme, spinner);
        // A boot console still going shows everything; once done it
        // folds and opens on its header.
        let toggles = message
            .boot_console()
            .filter(|console| !console.is_running())
            .map(|_| ToggleRow {
                line: 0,
                target: ToggleTarget::Step(message.version().0),
                open: message.is_expanded(),
            })
            .into_iter()
            .chain(more_toggles(message, more, 0))
            .collect();
        return (laid, toggles);
    };
    let mut toggles = vec![ToggleRow {
        line: 0,
        target: ToggleTarget::Step(message.version().0),
        open: message.is_expanded(),
    }];
    let (mut lines, more) = match kept {
        Some(kept) => {
            let mut drawn = StepLines::take(Some(kept), message, step);
            let lines = drawn.draw((message, step), None, true, (width, theme, spinner));
            kept.insert(message.version().0, drawn);
            lines
        }
        None => activity_lines(step, Shown::of(message), None, spinner, width, theme),
    };
    toggles.extend(more_toggles(message, more, 0));
    push_records_link(
        &mut lines,
        &mut toggles,
        (step, message.is_expanded()),
        None,
        place,
        (width, theme),
    );
    ((lines, Vec::new(), Vec::new()), toggles)
}

/// The rows of `message` that show more of a part cut short, `offset`
/// lines into its block.
fn more_toggles(
    message: &Message,
    more: MoreRows,
    offset: usize,
) -> impl Iterator<Item = ToggleRow> {
    let id = message.version().0;
    more.into_iter().map(move |(line, section)| ToggleRow {
        line: offset + line,
        target: ToggleTarget::More(id, section),
        open: false,
    })
}

/// The lines of one chat message, whether each is prose, the copy
/// buttons of its code blocks, and its rows that show more of a part cut
/// short. What the user sent follows a `> ` prompt and system messages a
/// `* `; the agent's replies stand on their own, like a program's output.
fn message_lines(
    message: &Message,
    (width, measure): (usize, Measure),
    theme: &Theme,
    spinner: &str,
) -> (Laid, MoreRows) {
    let alone = |lines| ((lines, Vec::new(), Vec::new()), MoreRows::new());
    let with_more = |(lines, more)| ((lines, Vec::new(), Vec::new()), more);
    if let Some(activity) = message.activity_step() {
        return with_more(activity_lines(
            activity,
            Shown::of(message),
            None,
            spinner,
            width,
            theme,
        ));
    }
    if let Some(run) = message.shell_run() {
        return alone(shell_lines(run, &message.timestamp_local(), width, theme));
    }
    if let Some(block) = message.diff_block() {
        let more = message.more_lines(MoreSection::Diff);
        return with_more(diff_message_lines(block, more, width, theme));
    }
    if let Some(console) = message.boot_console() {
        let (lines, more) = boot_lines(console, message.is_expanded(), spinner, width, theme);
        let copy = boot_copy(console, width).into_iter().collect();
        return ((lines, Vec::new(), copy), more);
    }
    let role = message.role();
    let (prompt, prompt_color, msg_color) = message_style(role, theme);
    if is_notice(message) {
        return alone(prefixed_lines(
            (prompt, prompt_color),
            message.content(),
            msg_color,
            width,
            Measure::GRID,
        ));
    }
    (
        content_lines(message.content(), role, (width, measure), theme),
        MoreRows::new(),
    )
}

/// The lines of the text a message of `role` says, whether each is prose,
/// and the copy buttons of its code blocks: fenced code set apart from
/// the text around it.
fn content_lines(
    content: &str,
    role: MessageRole,
    (width, measure): (usize, Measure),
    theme: &Theme,
) -> Laid {
    let (prompt, prompt_color, msg_color) = message_style(role, theme);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut prose = Vec::new();
    let mut codes = Vec::new();
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
                codes.extend(render_code_segment(
                    &mut lines, &language, &code, width, theme,
                ));
            }
        }
    }
    (lines, prose, codes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `turns` prompts, each answered with a tool call, a notice and a
    /// reply that has a list, code and a footer.
    fn long_chat(turns: usize) -> VecDeque<Message> {
        let mut chat = VecDeque::new();
        for turn in 0..turns {
            let prompt = format!("question {turn}: what is in `src/{turn}.rs`?");
            chat.push_back(Message::new(MessageRole::User, &prompt));
            let call = Activity::tool(&format!("call-{turn}"), "read_file", "{\"path\":\"a.rs\"}");
            let mut step = Message::activity(call, false);
            step.update_activity(|activity| activity.finish("read"));
            chat.push_back(step);
            let notice = format!("policy: read_file allowed by rule {turn}");
            chat.push_back(Message::new(MessageRole::System, &notice));
            let text = format!(
                "It holds **{turn}** things:\n\n- one\n- two, which goes on past the \
                 edge of a narrow pane\n\n```rust\nfn f() -> u32 {{ {turn} }}\n```\n"
            );
            let mut reply = Message::new(MessageRole::Assistant, &text);
            reply.set_model("claude-sonnet");
            chat.push_back(reply);
        }
        chat
    }

    #[test]
    fn a_chat_laid_out_across_threads_is_laid_out_as_it_is_a_message_at_a_time() {
        let theme = Theme::cyber();
        let open = HashMap::new();
        let groups = GroupState {
            open: &open,
            expand_all: false,
            turn_live: false,
        };
        let size = (40, Measure::GRID);
        let chat = long_chat(40);
        let mut whole = ChatLineCache::default();
        whole.update((&chat, &[]), size, &theme, "*", &groups);

        let mut grown = ChatLineCache::default();
        let mut so_far = VecDeque::new();
        for message in &chat {
            so_far.push_back(message.clone());
            grown.update((&so_far, &[]), size, &theme, "*", &groups);
        }
        let drawn = |cache: &ChatLineCache| {
            cache
                .blocks()
                .map(|block| {
                    (
                        block.lines.clone(),
                        block.prose.clone(),
                        block.toggles.len(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert!(whole.blocks().count() > PARALLEL_BLOCKS * 4);
        assert!(drawn(&whole) == drawn(&grown), "the same lines either way");
    }

    /// A `read_file` step that succeeded, or one still running.
    fn read_step(i: usize, done: bool) -> Message {
        let args = format!("{{\"path\":\"src/{i}.rs\"}}");
        let mut step = Message::activity(
            Activity::tool(&format!("call-{i}"), "read_file", &args),
            false,
        );
        if done {
            step.update_activity(|activity| {
                if let Activity::Tool(tool) = activity {
                    tool.status = crate::components::ToolStatus::Success;
                    tool.result = Some("fn main() {}".to_string());
                }
            });
        }
        step
    }

    #[test]
    fn a_live_group_drawn_from_its_kept_steps_matches_one_laid_out_afresh() {
        let theme = Theme::cyber();
        let open = HashMap::new();
        let groups = GroupState {
            open: &open,
            expand_all: false,
            turn_live: true,
        };
        let size = (60, Measure::GRID);
        let mut chat = VecDeque::from([Message::new(MessageRole::User, "look around")]);
        let mut kept = ChatLineCache::default();
        // A group's first line is its summary, drawn afresh every frame
        // with the clock of a running group, so it is left out: a second
        // passing between the two layouts would tell them apart.
        let lines = |cache: &ChatLineCache| {
            cache
                .blocks()
                .map(|block| {
                    let skip = usize::from(!block.user);
                    (block.lines[skip..].to_vec(), block.toggles.len())
                })
                .collect::<Vec<_>>()
        };
        // Past nine steps the numbers widen, which moves every step's
        // marker; one step opened midway and one failing change only
        // their own lines.
        for i in 0..12 {
            let mut step = read_step(i, i != 7);
            if i == 7 {
                step.update_activity(|activity| activity.finish("denied"));
            }
            chat.push_back(step);
            if i == 5 {
                if let Some(step) = chat.get_mut(3) {
                    step.set_expanded(true);
                }
            }
            kept.update((&chat, &[]), size, &theme, "*", &groups);
            let mut fresh = ChatLineCache::default();
            fresh.update((&chat, &[]), size, &theme, "*", &groups);
            assert!(lines(&kept) == lines(&fresh), "step {i}: the same lines");
        }
        let texts: Vec<String> = kept
            .blocks()
            .flat_map(|block| block.lines.iter().map(ToString::to_string))
            .collect();
        assert!(
            texts.iter().any(|line| line.contains("1 failed")),
            "{texts:#?}"
        );
        assert!(
            texts.iter().any(|line| line.contains("fn main() {}")),
            "the open step"
        );
    }

    #[test]
    fn a_live_group_keeps_its_finished_steps_but_not_running_ones() {
        let theme = Theme::cyber();
        let open = HashMap::new();
        let live = GroupState {
            open: &open,
            expand_all: false,
            turn_live: true,
        };
        let size = (60, Measure::GRID);
        let mut chat = VecDeque::from([
            Message::new(MessageRole::User, "look around"),
            read_step(0, true),
            read_step(1, true),
            read_step(2, false),
        ]);
        let mut cache = ChatLineCache::default();
        cache.update((&chat, &[]), size, &theme, "*", &live);
        let group = |cache: &ChatLineCache| {
            cache
                .blocks()
                .find(|block| !block.user)
                .map(|block| {
                    let mut steps: Vec<(u64, bool)> = block
                        .steps
                        .iter()
                        .map(|(id, step)| (*id, step.lines.is_some()))
                        .collect();
                    steps.sort_unstable();
                    steps
                })
                .unwrap_or_default()
        };
        let ids: Vec<u64> = chat.iter().skip(1).map(|m| m.version().0).collect();
        let mut expected = vec![(ids[0], true), (ids[1], true), (ids[2], false)];
        expected.sort_unstable();
        assert_eq!(
            group(&cache),
            expected,
            "a running step's spinner and clock move, so it is laid out every frame"
        );

        if let Some(step) = chat.back_mut() {
            step.update_activity(|activity| activity.finish("read"));
        }
        cache.update((&chat, &[]), size, &theme, "*", &live);
        assert!(group(&cache).iter().all(|&(_, kept)| kept));

        // Once the turn ends the group is cached whole and keeps no steps.
        let done = GroupState {
            turn_live: false,
            ..live
        };
        cache.update((&chat, &[]), size, &theme, "*", &done);
        assert!(group(&cache).is_empty());
    }

    /// A `write_file` call still running whose content has streamed in
    /// `bytes` long, its JSON not yet closed. It has no start time, so it
    /// shows no clock that could tick between two layouts.
    fn streaming_write(id: &str, bytes: usize) -> Message {
        let content = "fn main() {}\n".repeat(bytes / 13 + 1);
        let args = format!(
            "{{\"path\":\"src/{id}.rs\",\"content\":{}",
            serde_json::to_string(&content).unwrap_or_default()
        );
        let mut call = Activity::tool(id, "write_file", &args);
        if let Activity::Tool(tool) = &mut call {
            tool.started = None;
        }
        Message::activity(call, false)
    }

    #[test]
    fn a_running_tool_drawn_from_its_kept_parts_matches_one_laid_out_afresh() {
        let theme = Theme::cyber();
        let open = HashMap::new();
        let groups = GroupState {
            open: &open,
            expand_all: false,
            turn_live: true,
        };
        let size = (70, Measure::GRID);
        let lines = |cache: &ChatLineCache| {
            // A live group's summary line carries a clock (see above), and
            // is drawn afresh every frame anyway.
            cache
                .blocks()
                .flat_map(|block| block.lines.iter().map(ToString::to_string))
                .filter(|line| !line.contains(" · step "))
                .collect::<Vec<_>>()
        };
        // A lone step, then the same step at the end of a group, open
        // and closed, as the spinner turns.
        let mut chat = VecDeque::from([
            Message::new(MessageRole::User, "write it"),
            streaming_write("w0", 4_000),
        ]);
        let mut kept = ChatLineCache::default();
        for (frame, spinner) in ["⠋", "⠙", "⠹", "⠸"].into_iter().enumerate() {
            if frame == 2 {
                chat.insert(1, read_step(0, true));
                if let Some(step) = chat.back_mut() {
                    step.set_expanded(true);
                }
            }
            kept.update((&chat, &[]), size, &theme, spinner, &groups);
            let mut fresh = ChatLineCache::default();
            fresh.update((&chat, &[]), size, &theme, spinner, &groups);
            let (kept, fresh) = (lines(&kept), lines(&fresh));
            assert_eq!(kept, fresh, "frame {frame}");
            assert!(
                kept.iter()
                    .any(|line| line.contains(spinner) && line.contains(" KB")),
                "frame {frame}: the spinner turns on the running step\n{kept:#?}"
            );
        }
    }
}
