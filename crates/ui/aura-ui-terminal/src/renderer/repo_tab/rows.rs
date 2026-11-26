//! The rows of the REPO tab: the branch's sync, the uncommitted changes,
//! the timeline's events and the repo's branches, tags and worktrees.

use crate::app::repo_tab::RepoRow;
use crate::components::{cell_style, format_ago, list_row_with_detail, CellState, StatusIcon};
use crate::renderer::side_list::{name_style, Tone};
use crate::repo::{
    ChangeKind, EventKind, EventState, PrState, RepoBranch, RepoChange, RepoEvent, RepoView,
    RepoWorktree,
};
use crate::Theme;
use ratatui::{style::Color, text::Line};

/// What every row is drawn with.
#[derive(Debug, Clone, Copy)]
pub(super) struct Look {
    pub(super) width: usize,
    /// Now, in ms since the epoch, for each event's age
    pub(super) now: u64,
    /// Whether the changes are folded under their header
    pub(super) folded: bool,
    /// Whether the changes hang from a header, one level in
    pub(super) nested: bool,
}

/// What one row is drawn with: the list's [`Look`], the row's state and
/// the theme.
struct Pen<'a> {
    look: Look,
    state: CellState,
    theme: &'a Theme,
}

impl Pen<'_> {
    /// A row `depth` levels in: its icon, its name and detail in `tone`,
    /// and `end` on the right.
    fn line(
        &self,
        depth: usize,
        (glyph, color): (&str, Color),
        label: (&str, Option<&str>),
        tone: Tone,
        end: Option<&str>,
    ) -> Line<'static> {
        let cell = cell_style(self.theme, self.state, name_style(tone, self.theme));
        let icon = StatusIcon::new(glyph, color);
        list_row_with_detail(self.look.width, depth, icon, label, cell, end)
    }

    /// How long ago `at`, in seconds since the epoch, was; `None` when it
    /// is not known.
    fn ago(&self, at: i64) -> Option<String> {
        let then = u64::try_from(at).unwrap_or(0).saturating_mul(1000);
        (then > 0).then(|| format_ago(then, self.look.now))
    }

    /// How far in the changes sit.
    fn depth(&self) -> usize {
        usize::from(self.look.nested)
    }
}

pub(super) fn row_line(
    view: &RepoView,
    row: RepoRow,
    state: CellState,
    look: Look,
    theme: &Theme,
) -> Line<'static> {
    let pen = Pen { look, state, theme };
    let muted = theme.colors.muted;
    match row {
        RepoRow::Summary => summary_line(view, &pen),
        RepoRow::Changes => changes_line(view, &pen),
        RepoRow::Change(i) => view
            .changes
            .get(i)
            .map_or_else(Line::default, |change| change_line(change, &pen)),
        RepoRow::MoreChanges => {
            let name = format!("{} more", view.changes_more);
            let label = (name.as_str(), None);
            pen.line(pen.depth(), ("", muted), label, Tone::Settled, None)
        }
        RepoRow::PrNote => match &view.prs {
            PrState::Note(note) => {
                let label = (note.as_str(), None);
                pen.line(0, ("○", muted), label, Tone::Settled, None)
            }
            _ => Line::default(),
        },
        RepoRow::Event(i) => view
            .events
            .get(i)
            .map_or_else(Line::default, |event| event_line(event, &pen)),
        RepoRow::Branch(i) => view
            .branches
            .get(i)
            .map_or_else(Line::default, |branch| branch_line(branch, &pen)),
        RepoRow::Tag(i) => view.tags.get(i).map_or_else(Line::default, |tag| {
            let label = (tag.name.as_str(), Some(tag.subject.as_str()));
            pen.line(
                0,
                ("#", muted),
                label,
                Tone::Active,
                pen.ago(tag.at).as_deref(),
            )
        }),
        RepoRow::Worktree(i) => view
            .worktrees
            .get(i)
            .map_or_else(Line::default, |worktree| {
                worktree_line(view, worktree, &pen)
            }),
    }
}

/// The branch, how it stands with its upstream, and its head.
fn summary_line(view: &RepoView, pen: &Pen<'_>) -> Line<'static> {
    let name = view.branch.as_deref().unwrap_or("HEAD");
    let detail = sync_text(view);
    let end = Some(view.head.as_str()).filter(|head| !head.is_empty());
    let label = (name, Some(detail.as_str()));
    pen.line(0, sync_icon(view, pen.theme), label, Tone::Active, end)
}

/// The header the changes fold under, and how many files they are.
fn changes_line(view: &RepoView, pen: &Pen<'_>) -> Line<'static> {
    let fold = if pen.look.folded { "▸" } else { "▾" };
    let count = view.changes.len() + view.changes_more;
    let end = format!("{count} file{}", if count == 1 { "" } else { "s" });
    let name = ("Uncommitted changes", None);
    pen.line(
        0,
        (fold, pen.theme.colors.muted),
        name,
        Tone::Active,
        Some(end.as_str()),
    )
}

/// An uncommitted change, and how much of it is staged.
fn change_line(change: &RepoChange, pen: &Pen<'_>) -> Line<'static> {
    let name = change.from.as_ref().map_or_else(
        || change.path.clone(),
        |from| format!("{from} → {}", change.path),
    );
    let end = match (change.staged, change.unstaged) {
        (true, true) => Some("partly staged"),
        (true, false) => Some("staged"),
        _ => None,
    };
    let label = (name.as_str(), None);
    let icon = change_icon(change, pen.theme);
    pen.line(pen.depth(), icon, label, Tone::Active, end)
}

/// An event of the timeline and its age; merged and closed PRs read as
/// settled.
fn event_line(event: &RepoEvent, pen: &Pen<'_>) -> Line<'static> {
    let finished = matches!(event.state, EventState::Merged | EventState::Closed);
    let icon = event_icon(event.kind, event.state, pen.theme);
    let label = (event.title.as_str(), Some(event.detail.as_str()));
    pen.line(
        0,
        icon,
        label,
        Tone::active_if(!finished),
        pen.ago(event.at).as_deref(),
    )
}

/// A local branch: how far it is from its upstream, or that the upstream
/// is gone, and its last commit's age.
fn branch_line(branch: &RepoBranch, pen: &Pen<'_>) -> Line<'static> {
    let colors = &pen.theme.colors;
    let icon = if branch.current {
        ("●", colors.success)
    } else if branch.gone {
        ("✕", colors.muted)
    } else {
        ("○", colors.muted)
    };
    let track = if branch.gone {
        "gone".to_string()
    } else {
        track_text(branch.ahead, branch.behind)
    };
    let end = [Some(track), pen.ago(branch.at)]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("  ");
    let label = (branch.name.as_str(), Some(branch.subject.as_str()));
    pen.line(
        0,
        icon,
        label,
        Tone::active_if(!branch.gone),
        Some(end.as_str()),
    )
}

/// A worktree: its folder, its branch or detached head, and whether it is
/// locked or prunable.
fn worktree_line(view: &RepoView, worktree: &RepoWorktree, pen: &Pen<'_>) -> Line<'static> {
    let colors = &pen.theme.colors;
    let icon = if worktree.current {
        ("●", colors.success)
    } else if worktree.prunable {
        ("✕", colors.muted)
    } else {
        ("○", colors.muted)
    };
    // Worktrees sit side by side, so the folder they share is
    // left out.
    let path = view
        .root
        .parent()
        .and_then(|parent| worktree.path.strip_prefix(parent).ok())
        .unwrap_or(&worktree.path)
        .display()
        .to_string();
    let detail = worktree
        .branch
        .clone()
        .unwrap_or_else(|| format!("detached at {}", worktree.head));
    let end = if worktree.locked {
        Some("locked")
    } else if worktree.prunable {
        Some("prunable")
    } else {
        None
    };
    let tone = Tone::active_if(!worktree.prunable);
    pen.line(0, icon, (path.as_str(), Some(detail.as_str())), tone, end)
}

/// `↑2 ↓1`, `↑2`, `↓1`, or nothing when in sync.
fn track_text(ahead: usize, behind: usize) -> String {
    let parts = [(ahead, "↑"), (behind, "↓")]
        .into_iter()
        .filter(|&(n, _)| n > 0)
        .map(|(n, arrow)| format!("{arrow}{n}"));
    parts.collect::<Vec<_>>().join(" ")
}

/// How the branch stands with its upstream.
fn sync_text(view: &RepoView) -> String {
    let text = match (&view.branch, &view.upstream) {
        (None, _) => "detached".to_string(),
        (Some(_), None) => "no upstream".to_string(),
        (Some(_), Some(upstream)) => match (view.ahead, view.behind) {
            (0, 0) => format!("in sync with {upstream}"),
            (ahead, 0) => format!("{ahead} ahead of {upstream}"),
            (0, behind) => format!("{behind} behind {upstream}"),
            (ahead, behind) => format!("{ahead} ahead, {behind} behind {upstream}"),
        },
    };
    if view.worktree {
        format!("{text} · worktree")
    } else {
        text
    }
}

/// The summary's icon: which way the branch and its upstream are apart,
/// a check when they aren't.
const fn sync_icon(view: &RepoView, theme: &Theme) -> (&'static str, Color) {
    let colors = &theme.colors;
    if view.branch.is_none() {
        return ("◌", colors.muted);
    }
    if view.upstream.is_none() {
        return ("○", colors.muted);
    }
    match (view.ahead > 0, view.behind > 0) {
        (true, true) => ("↕", colors.warning),
        (true, false) => ("↑", colors.warning),
        (false, true) => ("↓", colors.warning),
        (false, false) => ("✓", colors.success),
    }
}

/// A change's status letter: in the accent once wholly staged, in
/// `warning` while some of it isn't, `error` on a conflict.
const fn change_icon(change: &RepoChange, theme: &Theme) -> (&'static str, Color) {
    let colors = &theme.colors;
    let color = match change.kind {
        ChangeKind::Conflicted => colors.error,
        ChangeKind::Untracked => colors.muted,
        _ if change.staged && !change.unstaged => colors.success,
        _ => colors.warning,
    };
    (change.kind.letter(), color)
}

/// An event's icon: unpushed commits are hollow, pushes and pulls point
/// the way the commits went, and a PR shows whether it is open, merged or
/// closed.
const fn event_icon(kind: EventKind, state: EventState, theme: &Theme) -> (&'static str, Color) {
    let colors = &theme.colors;
    match (kind, state) {
        (EventKind::Commit, EventState::Pending) => ("○", colors.warning),
        (EventKind::Commit, _) => ("●", colors.muted),
        (EventKind::Merge, _) => ("◆", colors.muted),
        (EventKind::Rebase, _) => ("◇", colors.muted),
        (EventKind::Push, _) => ("↑", colors.success),
        (EventKind::Pull, _) => ("↓", colors.success),
        (EventKind::Fetch, _) => ("↓", colors.muted),
        (EventKind::Reset, _) => ("↩", colors.warning),
        (EventKind::Checkout, _) => ("→", colors.muted),
        (EventKind::Stash, _) => ("▪", colors.muted),
        (EventKind::Pr, EventState::Merged) => ("✓", colors.success),
        (EventKind::Pr, EventState::Closed) => ("✕", colors.muted),
        (EventKind::Pr, EventState::Draft) => ("↪", colors.muted),
        (EventKind::Pr, _) => ("↪", colors.success),
    }
}
