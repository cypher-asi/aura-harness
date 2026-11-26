//! Laying chat blocks out, across threads when there are many.

use super::group::{group_lines, group_view, more_toggles, single_lines, KeptSteps};
use super::message::message_lines;
use super::stream::{self, Laid, Settled};
use super::{content_lines, BlockLines, GroupState, TurnPlace, USER_FRAME_ROWS};
use crate::app::{CodeCopyHit, ImageHit, ToggleRow};
use crate::components::{Message, MessageRole};
use crate::renderer::activity::GroupView;
use crate::renderer::command::shell_lines;
use crate::renderer::images::message_image_lines;
use crate::renderer::segments::{reply_footer, reply_meta};
use crate::renderer::text::Measure;
use crate::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use rayon::prelude::*;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::OnceLock;

/// Blocks to lay out at once from which they are laid out across
/// threads: fewer take less time than handing them out does.
pub(super) const PARALLEL_BLOCKS: usize = 16;

/// Most threads blocks are laid out on.
const MOST_LAYOUT_THREADS: usize = 8;

/// What every block of a frame is laid out with.
#[derive(Clone, Copy)]
pub(super) struct Layout<'a> {
    pub(super) terminal: bool,
    pub(super) prompt: &'a str,
    pub(super) width: usize,
    pub(super) measure: Measure,
    pub(super) theme: &'a Theme,
    pub(super) spinner: &'a str,
}

/// A block whose lines are out of date.
pub(super) struct Stale<'a> {
    /// The id of its first message
    pub(super) key: u64,
    pub(super) revision: u64,
    members: &'a [&'a Message],
    view: Option<GroupView>,
    pub(super) live: bool,
    place: TurnPlace<'a>,
}

impl<'a> Stale<'a> {
    /// The block of `members`, at `place` in its turn, and what it is drawn
    /// from; `last` when it is the chat's last block.
    pub(super) fn of(
        members: &'a [&'a Message],
        place: TurnPlace<'a>,
        groups: &GroupState<'_>,
        last: bool,
    ) -> Self {
        let key = members[0].version().0;
        let is_group = members.len() > 1 && members[0].activity_step().is_some();
        let live = members
            .iter()
            .any(|m| m.is_live_activity() || m.is_live_boot())
            || (is_group && groups.turn_live && last);
        let view =
            is_group.then(|| group_view(groups.open.get(&key).copied(), groups.expand_all, live));

        let mut hasher = std::hash::DefaultHasher::new();
        for member in members {
            member.version().hash(&mut hasher);
        }
        (view, live, &place).hash(&mut hasher);
        let revision = hasher.finish();
        Self {
            key,
            revision,
            members,
            view,
            live,
            place,
        }
    }
}

impl Layout<'_> {
    /// Whether `block` is a reply still being written, which is laid out
    /// from how it was the last time.
    pub(super) fn streams(self, block: &Stale<'_>) -> bool {
        !self.terminal
            && block.view.is_none()
            && block.members.len() == 1
            && stream::applies(block.members[0])
    }

    /// Whether `block` is a group of steps, or one step, still running,
    /// laid out from the steps it had the last time.
    pub(super) fn keeps_steps(self, block: &Stale<'_>) -> bool {
        block.live
            && (block.view.is_some()
                || (block.members.len() == 1 && block.members[0].activity_step().is_some()))
    }

    /// The lines of `blocks`, in order. A width or theme change, an agent
    /// switch or a chat's first frame lays out every block, which is
    /// shared out across threads.
    pub(super) fn blocks(self, blocks: &[Stale<'_>]) -> Vec<BlockLines> {
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
    pub(super) fn block(self, block: &Stale<'_>, before: Option<BlockLines>) -> BlockLines {
        let Body {
            laid: (mut lines, mut prose, mut codes),
            mut toggles,
            settled,
            steps,
        } = self.body(block, before);
        let members = block.members;
        if let Some(author) = members[0].author().filter(|_| members.len() == 1) {
            put_author_label(author, (&mut lines, &mut toggles, &mut prose), self.theme);
        }
        let mut images = self.push_images(members, &mut lines);
        if let Some(reply) = block.place.meta {
            push_footer(&mut lines, reply, self.width, self.theme);
        }
        let folded = members[0].inbox_fold().is_some() && !members[0].is_expanded();
        let user = !self.terminal
            && members.len() == 1
            && members[0].role() == MessageRole::User
            && !folded;
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

    /// What `block` is drawn as before its author's label, images, footer
    /// and frame.
    fn body(self, block: &Stale<'_>, before: Option<BlockLines>) -> Body {
        let members = block.members;
        let shell = members[0].shell_run().filter(|_| self.terminal);
        match (shell, block.view) {
            (Some(run), _) => Body::of((
                (
                    shell_lines(run, self.prompt, self.width, self.theme),
                    Vec::new(),
                    Vec::new(),
                ),
                Vec::new(),
            )),
            (None, Some(view)) => self.group_body(block, view, before),
            (None, None) if members.len() > 1 => Body::of(self.run_lines(members)),
            (None, None) if self.streams(block) => self.stream_body(block, before),
            (None, None) => self.single_body(block, before),
        }
    }

    /// A group of steps, keeping its steps while it is live.
    fn group_body(self, block: &Stale<'_>, view: GroupView, before: Option<BlockLines>) -> Body {
        // Only a live group is laid out again while it stays the
        // same, so only its steps are worth keeping.
        let kept = block
            .live
            .then(|| before.map(|block| block.steps).unwrap_or_default());
        let (lines, toggles, steps) = group_lines(
            block.members,
            (view, block.live),
            &block.place,
            (self.width, self.theme, self.spinner),
            kept,
        );
        Body {
            laid: (lines, Vec::new(), Vec::new()),
            toggles,
            settled: None,
            steps,
        }
    }

    /// A run of messages sharing a block, such as notices, one after another.
    fn run_lines(self, members: &[&Message]) -> (Laid, Vec<ToggleRow>) {
        let mut laid = Laid::default();
        let mut toggles = Vec::new();
        for member in members {
            let (more, rows) =
                message_lines(member, (self.width, self.measure), self.theme, self.spinner);
            toggles.extend(more_toggles(member, rows, laid.0.len()));
            stream::add((&mut laid.0, &mut laid.1, &mut laid.2), more);
        }
        (laid, toggles)
    }

    /// A reply still being written, going on from how it was laid out
    /// `before`.
    fn stream_body(self, block: &Stale<'_>, before: Option<BlockLines>) -> Body {
        let Self {
            width,
            measure,
            theme,
            ..
        } = self;
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
            block.members[0].content(),
            (&mut lines, &mut prose, &mut codes),
            |text| content_lines(text, MessageRole::Assistant, (width, measure), theme),
        );
        Body {
            laid: (lines, prose, codes),
            toggles: Vec::new(),
            settled: Some(state),
            steps: KeptSteps::new(),
        }
    }

    /// A block of one message.
    fn single_body(self, block: &Stale<'_>, before: Option<BlockLines>) -> Body {
        let members = block.members;
        // A lone step that runs is drawn again every frame; one
        // that is done is cached whole, so keeps nothing.
        let mut kept = (block.live && members[0].activity_step().is_some())
            .then(|| before.map(|block| block.steps).unwrap_or_default());
        let (laid, toggles) = single_lines(
            members[0],
            &block.place,
            (self.width, self.measure),
            self.theme,
            self.spinner,
            kept.as_mut(),
        );
        Body {
            laid,
            toggles,
            settled: None,
            steps: kept.unwrap_or_default(),
        }
    }

    /// The image thumbnails of a lone message, after its `lines`.
    fn push_images(self, members: &[&Message], lines: &mut Vec<Line<'static>>) -> Vec<ImageHit> {
        let mut images = Vec::new();
        if !self.terminal && members.len() == 1 && !members[0].images().is_empty() {
            let (more, hits) = message_image_lines(members[0], self.width, self.theme);
            let start = lines.len();
            images.extend(hits.into_iter().map(|hit| ImageHit {
                line: start + hit.line,
                ..hit
            }));
            lines.extend(more);
        }
        images
    }
}

/// A block's lines before its author's label, images, footer and frame,
/// its rows that toggle, how much of a streaming reply is settled, and
/// what a live block keeps of its steps.
struct Body {
    laid: Laid,
    toggles: Vec<ToggleRow>,
    settled: Option<Settled>,
    steps: KeptSteps,
}

impl Body {
    /// A body that settles and keeps nothing.
    fn of((laid, toggles): (Laid, Vec<ToggleRow>)) -> Self {
        Self {
            laid,
            toggles,
            settled: None,
            steps: KeptSteps::new(),
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
