//! The right end of an agent's row in the projects list and roster: its
//! buttons, time and code cells, shared by the renderer that draws them
//! and the hit-testing that finds the buttons.

use super::list_row::{INSET, ROW_INSET};
use crate::animation::AURA_WIDTH;
use unicode_width::UnicodeWidthStr;

/// Glyph of the archive button drawn on a hovered agent's row in the
/// projects list. The desktop draws it as a shape, so it lines up with the
/// cell grid there as well.
pub const AGENT_ARCHIVE_GLYPH: &str = "✕";

/// A button drawn on a hovered agent's row in the projects list. Clicking
/// elsewhere on the row already shows the agent, so there is no button
/// for that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentButton {
    /// Archives the agent.
    Archive,
}

impl AgentButton {
    /// The buttons, left to right as drawn.
    pub const ALL: [Self; 1] = [Self::Archive];

    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Archive => AGENT_ARCHIVE_GLYPH,
        }
    }

    /// What the button draws: a space, then its glyph.
    pub fn label(self) -> String {
        format!(" {}", self.glyph())
    }
}

/// The fewest columns an agent's name keeps beside its code; narrower rows
/// drop the code instead.
const MIN_NAME_BESIDE_CODE: usize = 4;

/// Columns before the name on the row of an agent at `depth`: the row's
/// inset, the spawn guides at `AURA_WIDTH + 2` columns per level, then the
/// icon column and the space after it, as `tree_row` lays them out.
fn agent_row_lead(depth: usize) -> usize {
    ROW_INSET + depth * (AURA_WIDTH + 2) + AURA_WIDTH + 1
}

/// What a projects-list or roster row shows right of its buttons, flush
/// with the right edge: the time cell, the code cell (on roster rows, the
/// definition's abbreviation), then the row's [`ROW_INSET`] blank columns.
///
/// The time cell is a space and the time (` 5m`), or nothing. The code
/// cell is a space and the agent's code, left-aligned and padded to the
/// code column's width so codes line up; with no code column it is empty.
///
/// The renderer draws these cells and the hit-testing finds the buttons
/// from them, so the two cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentRowEnd<'a> {
    ago: Option<&'a str>,
    code: Option<&'a str>,
    code_width: usize,
}

impl<'a> AgentRowEnd<'a> {
    /// The end of a row with a time and no code column, as project and
    /// archive rows have.
    pub const fn time(ago: Option<&'a str>) -> Self {
        Self {
            ago,
            code: None,
            code_width: 0,
        }
    }

    /// The end of an agent row: the time alone. Agent rows have no code
    /// column; only roster rows show a code (the definition's
    /// abbreviation), so names get the whole width.
    pub fn agent(ago: Option<&'a str>) -> Self {
        Self::time(ago)
    }

    /// The end of a roster row, in a list `width` columns wide whose
    /// abbreviation column is `column_width` columns (0 for none): no
    /// time, then the definition's `abbreviation` laid out as a code. The
    /// column is dropped when it would leave the name fewer than
    /// [`MIN_NAME_BESIDE_CODE`] columns. Roster rows have no buttons.
    pub fn definition(
        width: usize,
        (abbreviation, column_width): (Option<&'a str>, usize),
    ) -> Self {
        Self::coded_if_room(width, agent_row_lead(0), None, (abbreviation, column_width))
    }

    /// The end with `code` in a column `code_width` wide when the row,
    /// `width` columns with `taken` of them before the name and after it
    /// besides this end, leaves the name at least [`MIN_NAME_BESIDE_CODE`]
    /// columns; else the time alone.
    fn coded_if_room(
        width: usize,
        taken: usize,
        ago: Option<&'a str>,
        (code, code_width): (Option<&'a str>, usize),
    ) -> Self {
        if code_width == 0 {
            return Self::time(ago);
        }
        let with_code = Self {
            ago,
            code,
            code_width,
        };
        let around = taken + text_width(&with_code.text());
        if width.saturating_sub(around) >= MIN_NAME_BESIDE_CODE {
            with_code
        } else {
            Self::time(ago)
        }
    }

    /// The time cell: empty with no time.
    pub fn time_cell(&self) -> String {
        self.ago.map(|ago| format!(" {ago}")).unwrap_or_default()
    }

    /// The code cell: empty with no code column.
    pub fn code_cell(&self) -> String {
        if self.code_width == 0 {
            return String::new();
        }
        let code = self.code.unwrap_or_default();
        let pad = self.code_width.saturating_sub(text_width(code));
        format!(" {code}{}", " ".repeat(pad))
    }

    /// The whole text right of the buttons: the time cell, the code cell,
    /// then the row's inset.
    pub fn text(&self) -> String {
        self.time_cell() + &self.code_cell() + INSET
    }
}

fn text_width(text: &str) -> usize {
    text.width()
}

fn columns(text: &str) -> u16 {
    u16::try_from(text_width(text)).unwrap_or(u16::MAX)
}

/// Column of `button`'s glyph on an agent's row in the projects list,
/// counted from the row's left edge, for a row `width` columns wide that
/// ends in `end`. The renderer lays the row out from the same pieces:
/// each button's [`AgentButton::label`] in [`AgentButton::ALL`] order,
/// then [`AgentRowEnd::text`], flush with the right edge.
pub fn agent_button_offset(width: u16, end: &AgentRowEnd<'_>, button: AgentButton) -> u16 {
    let after: u16 = AgentButton::ALL
        .iter()
        .skip_while(|&&b| b != button)
        .skip(1)
        .map(|b| columns(&b.label()))
        .fold(0, u16::saturating_add);
    let glyph = columns(button.glyph());
    width.saturating_sub(
        columns(&end.text())
            .saturating_add(after)
            .saturating_add(glyph),
    )
}

/// The button at column `offset` of an agent's row laid out as
/// [`agent_button_offset`] says. Each button takes its glyph and the space
/// left of it, so the zones don't overlap; the last also takes the column
/// after it.
pub fn agent_button_at(width: u16, end: &AgentRowEnd<'_>, offset: u16) -> Option<AgentButton> {
    let last = AgentButton::ALL.len() - 1;
    AgentButton::ALL
        .iter()
        .enumerate()
        .find(|&(i, &button)| {
            let glyph = agent_button_offset(width, end, button);
            let end = glyph.saturating_add(columns(button.glyph()));
            let end = if i == last {
                end.saturating_add(1)
            } else {
                end
            };
            (glyph.saturating_sub(1)..end).contains(&offset)
        })
        .map(|(_, &button)| button)
}
