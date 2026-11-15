//! Where the cells of a panel's header bar fall: equal-width tab cells
//! between shared dividers, then an info cell filling the rest of the bar.
//!
//! ```text
//! ┌──────────┬──────────┬───────────┐
//! │   CHAT   │ TERMINAL │  PLANNER  │
//! └──────────┴──────────┴───────────┘
//! ```

/// How a bar's tab cells fit, roomiest first. Every tab in a bar uses the
/// same fit, so the cells stay the same width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabFit {
    /// The widest label with a space on each side
    Padded,
    /// The widest label, no padding
    Tight,
    /// Three-letter labels
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
    /// First column and width of the info cell, when there is room for one
    pub info: Option<(u16, u16)>,
}

/// A tab's label for one of the fits.
type Labels<'a, T> = &'a dyn Fn(T) -> &'static str;

/// Narrowest info cell drawn; less room than this widens the tabs instead.
const MIN_INFO_WIDTH: u16 = 3;

impl<T: Copy + PartialEq> TabLayout<T> {
    /// Lay out `tabs` in a bar `width` columns wide, padded if they fit,
    /// then tight, then with their short labels.
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
        let info_width = spare.saturating_sub(1);
        let info = (N == 0 || info_width >= MIN_INFO_WIDTH).then_some((used, info_width));
        let extra = if info.is_some() || N == 0 { 0 } else { spare };

        let mut start = 1;
        let cells = tabs
            .iter()
            .enumerate()
            .map(|(i, &tab)| {
                let index = u16::try_from(i).unwrap_or(u16::MAX);
                let width = cell + extra / count + u16::from(index < extra % count);
                let placed = TabCell {
                    tab,
                    label: text(tab),
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
    fn cells_are_as_wide_as_the_widest_label() {
        let wide = layout(60);
        assert_eq!(wide.fit, TabFit::Padded);
        assert_eq!(widths(&wide), [11, 11, 11]);
        assert_eq!(wide.cells[1].start, 13);
        assert_eq!(wide.info, Some((37, 22)));
    }

    #[test]
    fn narrow_bars_drop_padding_then_use_short_labels() {
        let side = layout(36);
        assert_eq!(side.fit, TabFit::Tight);
        assert_eq!(widths(&side), [9, 9, 9]);
        assert_eq!(side.info, Some((31, 4)));

        let narrow = layout(20);
        assert_eq!(narrow.fit, TabFit::Short);
        assert_eq!(narrow.cells[0].label, "SBX");
    }

    #[test]
    fn leftover_too_small_for_info_widens_the_tabs() {
        let tabs = layout(39);
        assert_eq!(tabs.fit, TabFit::Padded);
        assert_eq!(tabs.info, None);
        assert_eq!(widths(&tabs), [12, 12, 11]);
        let last = tabs.cells[2];
        assert_eq!(last.start + last.width, 38);
    }

    #[test]
    fn offsets_map_to_cells_but_not_dividers() {
        let tabs = layout(60);
        assert_eq!(tabs.tab_at(0), None);
        assert_eq!(tabs.tab_at(1), Some(0));
        assert_eq!(tabs.tab_at(12), None);
        assert_eq!(tabs.tab_at(13), Some(1));
        assert_eq!(tabs.tab_at(40), None);
    }

    #[test]
    fn untabbed_bars_are_all_info() {
        let bar = TabLayout::untabbed(30);
        assert!(bar.cells.is_empty());
        assert_eq!(bar.info, Some((1, 28)));
    }
}
