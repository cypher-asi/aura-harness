//! Groups of agent steps, and lone steps, as drawn.

use super::message::message_lines;
use super::stream::Laid;
use super::{StepLines, TurnPlace};
use crate::app::{ToggleRow, ToggleTarget};
use crate::components::{Activity, Message};
use crate::renderer::activity::{
    activity_lines, collapsed_hint, group_header_of, records_link, GroupView, Shown, StepNumber,
    Tally,
};
use crate::renderer::boot::boot_fold_rows;
use crate::renderer::text::Measure;
use crate::renderer::MoreRows;
use crate::Theme;
use ratatui::text::Line;
use std::collections::HashMap;

/// How much of a group to show: what the user chose, else everything under
/// `Ctrl+O` or while it runs.
pub(super) fn group_view(open: Option<bool>, expand_all: bool, live: bool) -> GroupView {
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
        progress.push_str(&crate::renderer::chat_panel::format_elapsed(elapsed));
    }
    progress
}

/// What a live group keeps of its steps from frame to frame, by the id of
/// each step's message.
pub(super) type KeptSteps = HashMap<u64, StepLines>;

/// A group's summary line, then every step, numbered, or a row to show
/// them while the group is collapsed.
///
/// A live group passes what it `kept` of its steps the last frame: a step
/// whose message has not changed since is drawn from that instead of
/// being laid out again (see [`StepLines::draw`]), and what the steps are
/// drawn as now is returned for the next frame.
pub(super) fn group_lines(
    members: &[&Message],
    (view, live): (GroupView, bool),
    place: &TurnPlace<'_>,
    look: (usize, &Theme, &str),
    mut kept: Option<KeptSteps>,
) -> (Vec<Line<'static>>, Vec<ToggleRow>, KeptSteps) {
    let (width, theme, spinner) = look;
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
        push_steps(
            (&mut lines, &mut toggles),
            (&steps, &mut drawn),
            (place, keeping),
            look,
        );
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

/// Every step of an open group, numbered from `place`'s first, after the
/// group's `lines`, with its rows that toggle. `drawn` holds what each
/// step kept, and keeps what it is drawn as now when `keeping`.
fn push_steps(
    (lines, toggles): (&mut Vec<Line<'static>>, &mut Vec<ToggleRow>),
    (steps, drawn): (&[(&Message, &Activity)], &mut [StepLines]),
    (place, keeping): (&TurnPlace<'_>, bool),
    (width, theme, spinner): (usize, &Theme, &str),
) {
    let mut number = place.first_step;
    let last = place.first_step + steps.len().saturating_sub(1);
    for (&(member, step), kept) in steps.iter().zip(drawn) {
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
            lines,
            toggles,
            (step, member.is_expanded()),
            Some(step_number),
            place,
            (width, theme),
        );
        number += 1;
    }
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
pub(super) fn single_lines(
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
        // folds, and opens or closes on its header, its outcome and the
        // row inviting the click.
        let id = message.version().0;
        let toggles = message
            .boot_console()
            .map_or(0..0, |console| {
                boot_fold_rows(console, message.is_expanded(), width)
            })
            .map(|line| ToggleRow {
                line,
                target: ToggleTarget::Step(id),
                open: message.is_expanded(),
            })
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
pub(super) fn more_toggles(
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
