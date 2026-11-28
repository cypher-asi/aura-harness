//! Chat lines for each message, kept between frames.

mod group;
mod layout;
mod message;
mod stream;
mod timeline;

use super::activity::{
    activity_lines, running_tool_lines, Shown, StepNumber, StepTally, ToolParts,
};
use super::text::Measure;
use super::MoreRows;
use crate::app::{CodeCopyHit, ImageHit, LinkHit, ToggleRow};
use crate::components::{Activity, Message};
use crate::events::ChannelPostView;
use crate::Theme;
use group::KeptSteps;
#[cfg(test)]
use layout::PARALLEL_BLOCKS;
use layout::{Layout, Stale};
use message::content_lines;
use ratatui::text::Line;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use stream::Settled;
pub(crate) use timeline::post_block_key;
use timeline::{block_runs, interleave_posts, post_message, turn_places};

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
    /// The rows of links a click opens, `line` counted from the block's
    /// first line, `x` in display columns, links numbered from 0 in the
    /// block
    pub links: Vec<LinkHit>,
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
        let reflowed = self.reflow(width, theme);
        let history = self.history(messages);
        let posts: &[ChannelPostView] = if self.terminal { &[] } else { posts };
        let post_messages: Vec<Message> = posts.iter().map(post_message).collect();
        let shown = interleave_posts(history, posts, &post_messages);
        let runs = block_runs(&shown);
        let places = self.places(&shown, &runs);
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
            let block = Stale::of(&shown[run.clone()], *place, groups, i + 1 == runs.len());
            self.order.push(block.key);
            if !block.live && self.is_current(&block) {
                continue;
            }
            if layout.streams(&block) || layout.keeps_steps(&block) {
                let before = self.entries.remove(&block.key);
                self.entries.insert(block.key, layout.block(&block, before));
            } else {
                stale.push(block);
            }
        }
        let laid = layout.blocks(&stale);
        self.store(&stale, laid);
        reflowed
    }

    /// Forget every block when the `width` or `theme` changed, returning
    /// whether they did.
    fn reflow(&mut self, width: usize, theme: &Theme) -> bool {
        let reflowed = self.width != width || self.theme.as_ref() != Some(theme);
        if reflowed {
            self.width = width;
            self.theme = Some(theme.clone());
            self.entries.clear();
        }
        reflowed
    }

    /// The messages of `messages` this pane draws.
    fn history<'m>(&self, messages: &'m VecDeque<Message>) -> Vec<&'m Message> {
        messages
            .iter()
            .filter(|m| m.shell_run().is_some() == self.terminal && !m.is_empty_reply())
            .collect()
    }

    /// Each block's place in its turn; the terminal pane has no turns.
    fn places<'a>(&self, shown: &[&'a Message], runs: &[Range<usize>]) -> Vec<TurnPlace<'a>> {
        if self.terminal {
            vec![TurnPlace::alone(); runs.len()]
        } else {
            turn_places(shown, runs)
        }
    }

    /// Whether the lines kept for `block` were drawn from what it is now.
    fn is_current(&self, block: &Stale<'_>) -> bool {
        self.entries
            .get(&block.key)
            .is_some_and(|kept| kept.revision == block.revision)
    }

    /// Keep the `laid` lines of the `stale` blocks, and forget blocks no
    /// longer shown.
    fn store(&mut self, stale: &[Stale<'_>], laid: Vec<BlockLines>) {
        for (block, lines) in stale.iter().zip(laid) {
            self.entries.insert(block.key, lines);
        }
        if self.entries.len() > self.order.len() {
            let live: HashSet<u64> = self.order.iter().copied().collect();
            self.entries.retain(|id, _| live.contains(id));
        }
    }

    /// Blocks in conversation order as of the last [`Self::update`]. A
    /// blank line goes between every two.
    pub(crate) fn blocks(&self) -> impl Iterator<Item = &BlockLines> {
        self.order.iter().filter_map(|key| self.entries.get(key))
    }

    /// The rows of the block keyed `key` (its first message's id), counted
    /// from the conversation's first row as [`Self::blocks`] lays them out.
    pub(crate) fn block_span(&self, key: u64) -> Option<Range<usize>> {
        let gap = self.block_gap();
        let mut row = 0;
        for (i, (block_key, block)) in self
            .order
            .iter()
            .filter_map(|k| self.entries.get(k).map(|block| (*k, block)))
            .enumerate()
        {
            if i > 0 {
                row += gap;
            }
            if block_key == key {
                return Some(row..row + block.lines.len());
            }
            row += block.lines.len();
        }
        None
    }
}

/// Blank rows above and below a user's message: the frame's border, then
/// a row of padding between it and the text.
pub(crate) const USER_FRAME_ROWS: usize = 2;

#[cfg(test)]
mod tests;
