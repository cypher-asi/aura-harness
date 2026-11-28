//! The plan's facts under its title: dim `label value` pairs, two
//! columns apart, packed onto as few rows as the width allows.
//!
//! ```text
//! created 14:32 · 2h ago  by RUS199 rust-engineer  asked by COO2 in #aura
//! status building 3/8  machine aura-rus199-12b4  branch aura/plan-meta
//! ```
//!
//! Who and when come first, then where the plan is and how far along;
//! each group starts a row of its own. A pair never splits across rows:
//! one that does not fit moves down whole, and one wider than the row has
//! its value cut with `…`. The rows stay on the cell grid.

use super::Laid;
use crate::app::{Fact, FactTone, PlanFacts};
use crate::components::truncate_record_info;
use crate::renderer::text::display_width;
use crate::Theme;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use std::hash::{DefaultHasher, Hash, Hasher};

/// Columns between two pairs on a row.
const PAIR_GAP: usize = 2;

/// A hash of `facts`, so a plan is laid out again when they change.
pub(super) fn key(facts: &PlanFacts) -> u64 {
    let mut hasher = DefaultHasher::new();
    facts.hash(&mut hasher);
    hasher.finish()
}

/// Add the rows of `facts` for `width` columns after a blank row; none
/// without facts.
pub(super) fn push_facts(laid: &mut Laid, facts: &PlanFacts, width: usize, theme: &Theme) {
    if facts.is_empty() {
        return;
    }
    laid.gap();
    for group in [&facts.who, &facts.state] {
        for line in fact_rows(group, width, theme) {
            laid.push(line);
        }
    }
}

/// `facts` packed onto rows `width` columns wide.
fn fact_rows(facts: &[Fact], width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for fact in facts {
        let label_width = display_width(fact.label);
        let room = width.saturating_sub(label_width + 1);
        let value = truncate_record_info(&fact.value, room);
        let pair = label_width + 1 + display_width(&value);
        if used > 0 && used + PAIR_GAP + pair > width {
            rows.push(Line::from(std::mem::take(&mut spans)));
            used = 0;
        }
        if used > 0 {
            spans.push(Span::raw(" ".repeat(PAIR_GAP)));
            used += PAIR_GAP;
        }
        spans.extend(pair_spans(fact, value, width.saturating_sub(used), theme));
        used += pair.min(width);
    }
    if !spans.is_empty() {
        rows.push(Line::from(spans));
    }
    rows
}

/// `label value`, the label `muted` and the value in its tone, cut to
/// `room` columns when even the label does not fit.
fn pair_spans(fact: &Fact, value: String, room: usize, theme: &Theme) -> Vec<Span<'static>> {
    let label_style = Style::default().fg(theme.colors.muted);
    let value_style = Style::default().fg(match fact.tone {
        FactTone::Plain => theme.colors.secondary,
        FactTone::Muted => theme.colors.muted,
        FactTone::Error => theme.colors.error,
    });
    let label_width = display_width(fact.label);
    if label_width + 1 >= room {
        let cut = truncate_record_info(&format!("{} {value}", fact.label), room);
        return vec![Span::styled(cut, label_style)];
    }
    vec![
        Span::styled(format!("{} ", fact.label), label_style),
        Span::styled(value, value_style),
    ]
}
