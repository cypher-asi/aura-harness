//! Rendering the tab in front into the grid and bringing the panels'
//! title bars and the tab bar up to date with it.

use super::{editor, Effect, Shell};
use crate::canvas::{self, Canvas};
use crate::chrome;
use crate::geom::Size;
use crate::grid::Palette;
use crate::router;
use crate::tab::Tab;
use crate::tab_bar;
use std::time::Instant;

impl Shell {
    /// Render the UI of the tab in front into the grid, then act on what
    /// the tabs asked for.
    pub(super) fn refresh(&mut self, now: Instant) {
        let area = self.grid_area();
        let (cols, rows) = self.metrics.grid(area);
        self.render_front(area, cols, rows);
        self.open_editors(now);
        self.update_bar(area, cols);
        self.save_tabs();
        self.selection.keep(self.tabs.last_selection());
    }

    /// Draw the terminal UI of the tab in front into its grid, `cols` by
    /// `rows` cells filling `area`, and keep what the frame shows.
    fn render_front(&mut self, area: Size, cols: u16, rows: u16) {
        let cell = self.metrics.cell;
        let tab = self.tabs.front_mut();
        if tab.tui.take_paste_request() {
            tab.tui.paste_from_clipboard();
        }
        let drawn = tab.terminal.backend().buffer().area;
        if (drawn.width, drawn.height) != (cols, rows) {
            let pixels = Size::new(f32::from(cols) * cell.width, f32::from(rows) * cell.height);
            tab.terminal.backend_mut().resize(cols, rows, pixels);
        }
        let (tui, tui_theme) = (&mut tab.tui, &self.tui_theme);
        tui.set_list_pitch(self.metrics.list_pitch());
        // This frame shows whatever changed so far.
        tui.take_redraw();
        // The grid backend's error type is `Infallible`.
        let Ok(frame) = tab
            .terminal
            .draw(|frame| aura_ui_terminal::render(frame, tui, tui_theme));
        if let Some(text) = tui.take_copy_text(frame.buffer) {
            self.effects.push(Effect::Copy(text));
        }
        self.cells.clone_from(&frame.buffer.content);
        (self.cols, self.rows) = (cols, rows);
        tui.prose_rows().clone_into(&mut self.prose);
        self.panes.clear();
        self.panes
            .extend(tui.pane_chrome().iter().map(|chrome| chrome.area));
        let at = (self.metrics.origin(area, cols), self.metrics, self.palette);
        let model = chrome::Model::new(tui.pane_chrome(), tui.side_toggle_area(), frame.buffer, at);
        self.chrome.update(model, self.pointer.on_grid);
        self.cursor = tab.terminal.backend().cursor();
    }

    /// Open the editors the tabs asked for.
    fn open_editors(&mut self, now: Instant) {
        for tab in self.tabs.iter_mut() {
            if let Some(path) = tab.tui.take_editor_request() {
                self.effects.extend(editor::open(tab, path, now));
            }
        }
    }

    /// Bring the tab bar up to date, its boxes lined up with the panels
    /// of the grid of `cols` columns in `area`.
    fn update_bar(&mut self, area: Size, cols: u16) {
        let cell = self.metrics.cell;
        let origin = self.metrics.origin(area, cols);
        // The panels' outer borders, which the UI keeps a column in from
        // the grid's edges, run through the middle of their cells; the
        // bar's boxes line up with them.
        let left = self.panes.iter().map(|pane| pane.x).min().unwrap_or(1);
        let right = self
            .panes
            .iter()
            .map(|pane| pane.right())
            .max()
            .unwrap_or_else(|| cols.saturating_sub(1))
            .max(left + 1);
        let model = tab_bar::Model {
            items: self
                .tabs
                .iter()
                .enumerate()
                .map(|(index, tab)| tab_bar::Item {
                    label: (index + 1).to_string(),
                    busy: tab.busy(),
                })
                .collect(),
            front: self.tabs.front_index(),
            title: self.tabs.front().title(),
            x: (origin.x + (f32::from(left) + 0.5) * cell.width).round(),
            width: f32::from(right - left - 1) * cell.width,
            metrics: self.metrics,
            palette: self.palette,
        };
        self.bar.update(model, self.pointer.on_bar);
    }

    /// Write the split of projects over tabs when it changed, once the
    /// session has listed its projects.
    fn save_tabs(&mut self) {
        let Some(path) = &self.tabs_file else {
            return;
        };
        let arrangement = self.tabs.arrangement();
        if arrangement == self.saved || arrangement.iter().any(Vec::is_empty) {
            return;
        }
        if let Err(e) = router::save_arrangement(path, &arrangement) {
            tracing::warn!(error = %e, path = %path.display(), "could not save the tabs");
        }
        self.saved = arrangement;
    }
}

/// The GRAPH tab's canvas, when `tab` shows it.
pub(super) fn canvas_of<'a>(
    tab: &'a Tab,
    theme: &aura_ui_terminal::Theme,
    palette: Palette,
) -> Option<Canvas<'a>> {
    let tui = &tab.tui;
    tui.flow_canvas().map(|area| Canvas {
        flow: tui.flow(),
        area,
        clip: tui.flow_canvas_clip(),
        colors: canvas::Colors::new(theme, palette),
    })
}
