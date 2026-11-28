//! The second row of a two-row list item: a muted line under the name of
//! the row above, on the same fill. A channel's MEMBERS draws what each
//! member is doing on it, and HISTORY when and where each message went,
//! so both read the same way.
//!
//! ```text
//!  @  rust-engineer             RE36     list_row / tree_row
//!     Fix parser escapes                 detail_row
//!     └── subagent              SB12
//!     │   Write the escape tests         detail_row under guides
//! ```

use super::list_row::{truncate_record_info, ROW_INSET};
use crate::animation::AURA_WIDTH;
use crate::themes::Theme;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

/// A row `width` columns wide: the row's inset, `guides` (the tree lines
/// running on under the row above, in `muted`), then `text` where the
/// name above starts, cut to fit, on `bg` in the `text` style. The row
/// keeps its blanks at the right end, as the name's row does.
pub fn detail_row<'a>(
    width: usize,
    guides: &str,
    text: &str,
    (bg, style): (Option<Color>, Style),
    theme: &Theme,
) -> Line<'a> {
    let fill = bg.map_or_else(Style::default, |bg| Style::default().bg(bg));
    let lead = format!("{:ROW_INSET$}{guides}{}", "", " ".repeat(AURA_WIDTH + 1));
    let lead_width = lead.width();
    let text = truncate_record_info(text, width.saturating_sub(lead_width + ROW_INSET));
    let padding = width.saturating_sub(lead_width + text.width());
    Line::from(vec![
        Span::styled(lead, fill.fg(theme.colors.muted)),
        Span::styled(text, fill.patch(style)),
        Span::styled(" ".repeat(padding), fill),
    ])
}

#[cfg(test)]
mod tests {
    use super::super::list_row::CellStyle;
    use super::*;
    use crate::components::{list_row, StatusIcon};

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| &*span.content).collect()
    }

    fn plain(theme: &Theme) -> CellStyle {
        crate::components::cell_style(
            theme,
            crate::components::CellState::Normal,
            Style::default().fg(theme.colors.foreground),
        )
    }

    #[test]
    fn the_detail_starts_under_the_name_and_fills_the_row() {
        let theme = Theme::default();
        let name = list_row(
            24,
            0,
            StatusIcon::new("@", theme.colors.muted),
            "coder",
            plain(&theme),
            None,
        );
        let detail = detail_row(
            24,
            "",
            "Fix the parser",
            (None, Style::default().fg(theme.colors.muted)),
            &theme,
        );
        let (name, detail) = (text(&name), text(&detail));
        assert_eq!(
            name.find("coder"),
            detail.find("Fix"),
            "{name:?} {detail:?}"
        );
        assert_eq!(detail.width(), 24);
    }

    #[test]
    fn a_long_detail_is_cut_and_the_fill_runs_the_whole_row() {
        let theme = Theme::default();
        let bg = Some(theme.colors.selection);
        let line = detail_row(
            16,
            "│ ",
            "Write the escape tests",
            (bg, Style::default()),
            &theme,
        );
        assert!(text(&line).contains('…'), "{:?}", text(&line));
        assert_eq!(text(&line).width(), 16);
        assert!(line.spans.iter().all(|span| span.style.bg == bg));
    }
}
