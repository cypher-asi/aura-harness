//! Where the cells of a panel's header bar fall: equal-width tab cells
//! between shared dividers, filling the bar. A bar with no tabs is one info
//! cell.
//!
//! ```text
//! ┌───────────┬───────────┬───────────┐
//! │   CHAT    │ TERMINAL  │  PLANNER  │
//! └───────────┴───────────┴───────────┘
//! ```

/// How a bar's tab cells fit, roomiest first. Every tab in a bar uses the
/// same fit, so the cells stay the same width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabFit {
    /// The widest label with a space on each side
    Padded,
    /// The widest label, no padding
    Tight,
    /// Three-letter labels, except in cells wide enough for the full one
    Short,
}

/// One tab's cell, in columns from the bar's left edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabCell<T> {
    pub tab: T,
    /// Text drawn in the cell, for the bar's fit
    pub label: &'static str,
    pub start: u16,
    pub width: u16,
}

/// The cells of a header bar `width` columns wide. Column 0, the last
/// column and the column after each cell hold the frame or a divider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabLayout<T> {
    pub fit: TabFit,
    pub cells: Vec<TabCell<T>>,
    /// First column and width of the info cell, for bars without tabs
    pub info: Option<(u16, u16)>,
}

/// A tab's label for one of the fits.
type Labels<'a, T> = &'a dyn Fn(T) -> &'static str;

impl<T: Copy + PartialEq> TabLayout<T> {
    /// Lay out `tabs` in a bar `width` columns wide, padded if they fit,
    /// then tight, then with their short labels. Columns left over widen
    /// the cells so the tabs span the bar.
    pub fn new<const N: usize>(
        tabs: [T; N],
        label: impl Fn(T) -> &'static str,
        short: impl Fn(T) -> &'static str,
        width: u16,
    ) -> Self {
        let count = u16::try_from(N).unwrap_or(u16::MAX);
        let widest = |text: Labels<'_, T>| {
            tabs.iter()
                .map(|&tab| u16::try_from(text(tab).len()).unwrap_or(u16::MAX))
                .max()
                .unwrap_or(0)
        };
        let fits: [(TabFit, Labels<'_, T>, u16); 3] = [
            (TabFit::Padded, &label, 2),
            (TabFit::Tight, &label, 0),
            (TabFit::Short, &short, 0),
        ];
        let (fit, text, cell) = fits
            .iter()
            .map(|&(fit, text, pad)| (fit, text, widest(text) + pad))
            .find(|&(_, _, cell)| count * (cell + 1) < width)
            .unwrap_or_else(|| {
                let (fit, text, _) = fits[2];
                (fit, text, widest(text))
            });

        let used = 1 + count * (cell + 1);
        let spare = width.saturating_sub(used);
        // The info cell also needs the frame's right border after it.
        let info = (N == 0).then_some((used, spare.saturating_sub(1)));
        let extra = if N == 0 { 0 } else { spare };

        let mut start = 1;
        let cells = tabs
            .iter()
            .enumerate()
            .map(|(i, &tab)| {
                let index = u16::try_from(i).unwrap_or(u16::MAX);
                let width = cell + extra / count + u16::from(index < extra % count);
                let full = label(tab);
                let placed = TabCell {
                    tab,
                    label: if full.len() <= usize::from(width) {
                        full
                    } else {
                        text(tab)
                    },
                    start,
                    width,
                };
                start = start.saturating_add(width + 1);
                placed
            })
            .collect();
        Self { fit, cells, info }
    }

    /// The tab whose cell covers `offset` columns from the bar's left edge.
    pub fn tab_at(&self, offset: u16) -> Option<T> {
        self.cells
            .iter()
            .find(|cell| offset >= cell.start && offset < cell.start + cell.width)
            .map(|cell| cell.tab)
    }
}

impl TabLayout<()> {
    /// A bar with no tabs: the info cell spans it.
    pub fn untabbed(width: u16) -> Self {
        Self::new([], |()| "", |()| "", width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LABELS: [&str; 3] = ["SANDBOX", "PROCESSES", "TRACE"];
    const SHORT: [&str; 3] = ["SBX", "PRC", "TRC"];

    fn layout(width: u16) -> TabLayout<usize> {
        TabLayout::new([0, 1, 2], |i| LABELS[i], |i| SHORT[i], width)
    }

    fn widths(layout: &TabLayout<usize>) -> Vec<u16> {
        layout.cells.iter().map(|cell| cell.width).collect()
    }

    #[test]
    fn tabs_share_the_whole_bar() {
        let wide = layout(60);
        assert_eq!(wide.fit, TabFit::Padded);
        assert_eq!(widths(&wide), [19, 19, 18]);
        assert_eq!(wide.cells[1].start, 21);
        assert_eq!(wide.info, None);
        let last = wide.cells[2];
        assert_eq!(last.start + last.width, 59);
    }

    #[test]
    fn narrow_bars_drop_padding_then_use_short_labels() {
        let side = layout(36);
        assert_eq!(side.fit, TabFit::Tight);
        assert_eq!(widths(&side), [11, 11, 10]);

        let narrow = layout(20);
        assert_eq!(narrow.fit, TabFit::Short);
        assert_eq!(narrow.cells[0].label, "SBX");
        let last = narrow.cells[2];
        assert_eq!(last.start + last.width, 19);
    }

    #[test]
    fn short_bars_keep_full_labels_that_fit_their_cell() {
        let bar = layout(28);
        assert_eq!(bar.fit, TabFit::Short);
        assert_eq!(widths(&bar), [8, 8, 8]);
        let labels: Vec<&str> = bar.cells.iter().map(|cell| cell.label).collect();
        assert_eq!(labels, ["SANDBOX", "PRC", "TRACE"]);
    }

    #[test]
    fn offsets_map_to_cells_but_not_dividers() {
        let tabs = layout(60);
        assert_eq!(tabs.tab_at(0), None);
        assert_eq!(tabs.tab_at(1), Some(0));
        assert_eq!(tabs.tab_at(20), None);
        assert_eq!(tabs.tab_at(21), Some(1));
        assert_eq!(tabs.tab_at(41), Some(2));
        assert_eq!(tabs.tab_at(59), None);
    }

    #[test]
    fn untabbed_bars_are_all_info() {
        let bar = TabLayout::untabbed(30);
        assert!(bar.cells.is_empty());
        assert_eq!(bar.info, Some((1, 28)));
    }
}
