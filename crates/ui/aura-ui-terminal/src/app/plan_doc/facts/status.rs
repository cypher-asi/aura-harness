//! A plan's status, worked out from what the app holds.
//!
//! The software factory's own stages stay in the session, so the view
//! reads the same steps from their traces, first match wins:
//!
//! | what the app sees                                   | status     |
//! |-----------------------------------------------------|------------|
//! | the author's merge entry is queued or running       | `shipping` |
//! | the entry merged                                    | `merged`   |
//! | the entry failed                                    | `failed`   |
//! | every todo closed, by a channel member              | `shipping` |
//! | every todo closed                                   | `done`     |
//! | the author is in plan mode                          | `planning` |
//! | a todo in progress or closed                        | `building` |
//! | anything else                                       | `planned`  |
//!
//! The todo progress, such as `3/8`, follows the status.

use super::{Fact, FactTone};
use crate::app::PlanDoc;
use crate::events::{AgentSummary, ChannelSummary, MergeEntryView, MergeStateView, TodoState};

/// The `status` fact of `doc`, by `author` of `channel` with its merge
/// queue `entry`.
pub(crate) fn plan_status(
    doc: &PlanDoc,
    author: Option<&AgentSummary>,
    channel: Option<&ChannelSummary>,
    entry: Option<&MergeEntryView>,
) -> Fact {
    let (closed, total) = doc.progress();
    let started = doc
        .todos
        .iter()
        .any(|todo| todo.state != TodoState::Pending);
    let status = match entry.map(|entry| entry.state) {
        Some(MergeStateView::Queued | MergeStateView::Running) => "shipping",
        Some(MergeStateView::Merged) => "merged",
        Some(MergeStateView::Failed) => "failed",
        None if total > 0 && closed == total && channel.is_some() => "shipping",
        None if total > 0 && closed == total => "done",
        None if author.is_some_and(|agent| agent.plan_mode) => "planning",
        None if started => "building",
        None => "planned",
    };
    let value = if total > 0 {
        format!("{status} {closed}/{total}")
    } else {
        status.to_string()
    };
    Fact {
        label: "status",
        value,
        tone: if status == "failed" {
            FactTone::Error
        } else {
            FactTone::Plain
        },
    }
}
