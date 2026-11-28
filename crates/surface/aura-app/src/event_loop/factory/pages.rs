//! How much of a repo's merge history each channel's QUEUE tab shows.
//!
//! A tab starts with the newest [`PAGE`] finished merges (and every
//! waiting or running one), and each `LoadOlderMerges` from it widens its
//! window by another page. Windows live as long as the app; they only
//! ever grow, so a tab never loses what it scrolled to.

use super::view::queue_view;
use aura_exec_merge_queue::MergeQueue;
use aura_ui::events::MergeQueueView;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Mutex;

/// Finished merges a tab is sent at a time.
pub(crate) const PAGE: usize = 100;

/// Each channel's window: how many finished merges its tab shows.
#[derive(Debug, Default)]
pub(crate) struct Windows {
    shown: Mutex<HashMap<String, usize>>,
}

impl Windows {
    fn shown(&self) -> std::sync::MutexGuard<'_, HashMap<String, usize>> {
        self.shown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// How many finished merges channel `channel_id`'s tab shows.
    pub(crate) fn of(&self, channel_id: &str) -> usize {
        self.shown().get(channel_id).copied().unwrap_or(PAGE)
    }

    /// Show channel `channel_id`'s tab another page of older merges.
    pub(crate) fn widen(&self, channel_id: &str) {
        let mut shown = self.shown();
        let window = shown.entry(channel_id.to_string()).or_insert(PAGE);
        *window = window.saturating_add(PAGE);
    }
}

/// The view each of `channels` gets of `queue`, merging into `target`,
/// at its own window, with the repo's page on GitHub for QUEUE to link
/// branches under; the queue is read once per distinct window.
pub(crate) async fn views(
    queue: &MergeQueue,
    target: &str,
    channels: Vec<String>,
    windows: &Windows,
) -> Vec<(String, MergeQueueView)> {
    let top = &queue.repo().top;
    let repo = top.display().to_string();
    let web = crate::event_loop::repo::github_page(top).await;
    let mut read: HashMap<usize, MergeQueueView> = HashMap::new();
    let mut out = Vec::with_capacity(channels.len());
    for channel_id in channels {
        let window = windows.of(&channel_id);
        let view = match read.entry(window) {
            Entry::Occupied(known) => known.get().clone(),
            Entry::Vacant(slot) => {
                let page = queue.page(window).await;
                let mut view = queue_view(target, &repo, &page.entries, page.older, web.clone());
                view.broken = page.held;
                slot.insert(view).clone()
            }
        };
        out.push((channel_id, view));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_starts_at_a_page_and_grows_a_page_at_a_time() {
        let windows = Windows::default();
        assert_eq!(windows.of("c1"), PAGE);
        windows.widen("c1");
        windows.widen("c1");
        assert_eq!(windows.of("c1"), 3 * PAGE);
        assert_eq!(windows.of("c2"), PAGE, "each channel its own");
    }
}
