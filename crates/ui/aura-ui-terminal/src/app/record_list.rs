//! The Kernel panel's records list: the records, the highlighted one, its
//! scroll and which list the panel shows.

use super::RecordsTab;
use crate::events::RecordSummary;
use std::collections::VecDeque;

/// The kernel records list, its selection, scroll and tab.
#[derive(Debug, Default)]
pub(crate) struct RecordList {
    /// Kernel records list
    pub(crate) items: VecDeque<RecordSummary>,
    /// Selected record index in the list
    pub(crate) selected: usize,
    /// Scroll offset for records list
    pub(crate) scroll: usize,
    /// Which list the Kernel panel shows
    pub(crate) tab: RecordsTab,
}
