//! The QUEUE tab's history, a page at a time.
//!
//! The session sends a channel's queue as what waits and runs plus the
//! newest page of finished merges, with a cursor to the older ones
//! (`older`). Scrolled to its end, the tab asks for the page before that
//! cursor (`UiEvent::LoadMergeHistory`), once per cursor, and the page
//! comes back as a view with `page_before` set, whose merges go after
//! those shown. A fresh view (the queue changed) replaces the newest
//! page but keeps the older pages already loaded, so a change never
//! throws the user back to the top of a long history.

use crate::app::App;
use crate::events::{MergeQueueView, UiEvent};
use std::collections::HashSet;

/// How far one channel's QUEUE tab has paged into its history.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct QueuePages {
    /// Whether pages past the newest were added to the view
    extended: bool,
    /// The cursor last asked for, so it is asked once
    asked: Option<u64>,
}

impl App {
    /// Keep the merge queue of channel `channel_id`'s repo, for its QUEUE
    /// tab: a page of older merges goes after those shown; a fresh view
    /// takes the place of the one before, keeping older pages loaded.
    pub(in crate::app) fn set_merge_queue(&mut self, channel_id: String, queue: MergeQueueView) {
        let pages = self
            .channels
            .queue_pages
            .entry(channel_id.clone())
            .or_default();
        let shown = self.channels.merge_queues.remove(&channel_id);
        let view = match (queue.page_before, shown) {
            (Some(before), Some(shown)) => add_page(shown, queue, before, pages),
            // A page for a view no longer held: nothing to add it to.
            (Some(_), None) => return,
            (None, Some(shown)) if pages.extended => keep_older(queue, shown),
            (None, _) => {
                *pages = QueuePages::default();
                queue
            }
        };
        self.channels.merge_queues.insert(channel_id, view);
    }

    /// Ask for the open channel's next page of older merges, once per
    /// cursor, when its view has more.
    pub(crate) fn request_older_merges(&mut self) {
        let Some(id) = self.channels.open.clone() else {
            return;
        };
        let Some(before) = self.channels.merge_queues.get(&id).and_then(|q| q.older) else {
            return;
        };
        let pages = self.channels.queue_pages.entry(id.clone()).or_default();
        if pages.asked == Some(before) {
            return;
        }
        pages.asked = Some(before);
        let asked = self.try_send_event(UiEvent::LoadMergeHistory {
            channel_id: id.clone(),
            before,
        });
        if !asked {
            // Try again on the next frame.
            if let Some(pages) = self.channels.queue_pages.get_mut(&id) {
                pages.asked = None;
            }
        }
    }
}

/// `shown` with `page`, the merges before cursor `before`, after its own;
/// a page for another cursor is stale and changes nothing.
fn add_page(
    mut shown: MergeQueueView,
    page: MergeQueueView,
    before: u64,
    pages: &mut QueuePages,
) -> MergeQueueView {
    if shown.older != Some(before) {
        return shown;
    }
    let known: HashSet<String> = shown.entries.iter().map(|e| e.id.clone()).collect();
    let new = page.entries.into_iter().filter(|e| !known.contains(&e.id));
    shown.entries.extend(new);
    shown.older = page.older;
    pages.extended = true;
    shown
}

/// `fresh`, the queue now, followed by the older merges `shown` had
/// loaded past its newest page, and its cursor.
fn keep_older(mut fresh: MergeQueueView, shown: MergeQueueView) -> MergeQueueView {
    let known: HashSet<String> = fresh.entries.iter().map(|e| e.id.clone()).collect();
    let older = shown
        .entries
        .into_iter()
        .filter(|e| e.state.is_finished() && !known.contains(&e.id));
    fresh.entries.extend(older);
    fresh.older = shown.older;
    fresh
}

#[cfg(test)]
mod tests;
