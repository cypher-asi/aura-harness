//! The progress bar of a running boot console.

use super::{clip, BODY};
use crate::components::BootConsole;
use crate::events::BootStageState;
use crate::renderer::text::display_width;
use crate::Theme;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

/// Widest the progress bar gets.
const MAX_BAR_CELLS: usize = 48;

/// Milliseconds the band of light over a running step takes per cell.
const SWEEP_MS: u128 = 80;

/// Cells' worth of time the band waits off a step before passing again.
const BAND_GAP: u128 = 6;

/// A cell of the bar: its glyph and color, and the running step it shades.
type BarCell = (char, Color, Option<usize>);

/// `██████▓▒▒░░░░░░░  36%`: as a benchmark's bar, a run of cells per
/// step, those finished solid in the color of how they ended, the running
/// ones filled as far as they've got and shaded beyond with a band of
/// light passing, those to come faint.
pub(super) fn bar_line(console: &BootConsole, width: usize, theme: &Theme) -> Line<'static> {
    let percent = (console.fraction() * 100.0).floor().min(99.0);
    let label = format!("  {percent:>3}%");
    let cells = width
        .saturating_sub(BODY.len() + display_width(&label))
        .clamp(4, MAX_BAR_CELLS);
    let mut look = bar_cells(console, cells, theme);
    let tick = console.started.elapsed().as_millis() / SWEEP_MS;
    sweep_bands(&mut look, tick, theme);
    let mut spans = bar_spans(&look);
    spans.push(Span::styled(label, Style::default().fg(theme.colors.text)));
    clip(Line::from(spans), width)
}

/// The bar's `cells` cells, before the band of light passes over them.
fn bar_cells(console: &BootConsole, cells: usize, theme: &Theme) -> Vec<BarCell> {
    let steps = console.steps.len();
    (0..cells)
        .map(|i| {
            if steps == 0 {
                return ('▒', theme.colors.pending, Some(0));
            }
            #[allow(clippy::cast_precision_loss)]
            let at = (i as f64 + 0.5) / cells as f64 * steps as f64;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let index = (at as usize).min(steps - 1);
            let step = &console.steps[index];
            #[allow(clippy::cast_precision_loss)]
            let within = at - index as f64;
            match &step.state {
                BootStageState::Pending => ('░', theme.colors.faint, None),
                BootStageState::Running if within < step.fraction() => {
                    ('█', theme.colors.success, None)
                }
                BootStageState::Running => ('▒', theme.colors.pending, Some(index)),
                BootStageState::Done { .. } => ('█', theme.colors.success, None),
                BootStageState::Skipped(_) => ('█', theme.colors.muted, None),
                BootStageState::Failed(_) => ('█', theme.colors.error, None),
            }
        })
        .collect()
}

/// Light the cell the band has reached, `tick` cells in, on each running
/// step's shaded run.
fn sweep_bands(look: &mut [BarCell], tick: u128, theme: &Theme) {
    let mut i = 0;
    while i < look.len() {
        let Some(step) = look[i].2 else {
            i += 1;
            continue;
        };
        let len = look[i..].iter().take_while(|c| c.2 == Some(step)).count();
        #[allow(clippy::cast_possible_truncation)]
        let band = (tick % (len as u128 + BAND_GAP)) as usize;
        if band < len {
            look[i + band].0 = '▓';
            look[i + band].1 = theme.colors.text;
        }
        i += len;
    }
}

/// The cells as spans after the body's indent, one per run of a color.
fn bar_spans(look: &[BarCell]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = vec![Span::raw(BODY)];
    let mut run = String::new();
    let mut color = None;
    for &(glyph, fg, _) in look {
        if color.is_some_and(|c| c != fg) {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                Style::default().fg(color.unwrap_or(fg)),
            ));
        }
        color = Some(fg);
        run.push(glyph);
    }
    if let Some(color) = color {
        spans.push(Span::styled(run, Style::default().fg(color)));
    }
    spans
}
