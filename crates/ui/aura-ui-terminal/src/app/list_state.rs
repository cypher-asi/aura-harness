//! A list of items with a highlighted row and a scroll position.

/// Items of a list, the highlighted one and the first row shown.
#[derive(Debug, Clone)]
pub(crate) struct ListState<T> {
    /// The items, in list order
    pub(crate) items: Vec<T>,
    /// Index of the highlighted item; a list may let it sit past the
    /// last item, on a row of its own
    pub(crate) selected: usize,
    /// First visible row
    pub(crate) scroll: usize,
}

impl<T> Default for ListState<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            selected: 0,
            scroll: 0,
        }
    }
}

impl<T> ListState<T> {
    /// Number of items.
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether there are no items.
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The highlighted item, if the highlight is on one.
    pub(crate) fn selected_item(&self) -> Option<&T> {
        self.items.get(self.selected)
    }
}
