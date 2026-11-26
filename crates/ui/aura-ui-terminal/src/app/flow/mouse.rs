//! The mouse over the GRAPH tab: wheel, clicks, drags and hovering.

use super::{cells, FlowMode, WHEEL_COLUMNS};
use crate::app::{App, PanelFocus};
use crossterm::event::{KeyModifiers, MouseEventKind};

impl App {
    /// Move the GRAPH tab's view `dx` × `dy` cells over its canvas.
    pub fn pan_flow(&mut self, dx: f32, dy: f32) {
        self.flow.pan_by(dx, dy);
    }

    /// Zoom the GRAPH tab's canvas by `factor` around `anchor`, in screen
    /// cells from the top left of [`Self::flow_canvas`] (its middle for
    /// `None`). Only a front end that draws the canvas zooms it.
    pub fn zoom_flow(&mut self, factor: f32, anchor: Option<(f32, f32)>) {
        if self.host.canvas {
            self.flow.zoom_at(factor, anchor);
        }
    }

    /// A click at canvas point `(x, y)`, in cells: see
    /// [`Self::click_flow`]. Returns whether it was on a card.
    pub fn click_flow_at(&mut self, x: f32, y: f32) -> bool {
        self.flow.ensure_layout();
        let Some(spot) = self.flow.layout.spot_at(x, y) else {
            return false;
        };
        self.focus = PanelFocus::Chat;
        if self.flow.click(spot) {
            self.open_flow_node();
        }
        true
    }

    /// The pointer is at canvas point `at` of the GRAPH tab, in cells, or
    /// off its canvas for `None`. Returns whether a fold button under it
    /// is to be drawn differently.
    pub fn hover_flow_at(&mut self, at: Option<(f32, f32)>) -> bool {
        self.flow.ensure_layout();
        self.flow.hover(at)
    }

    /// The wheel over the GRAPH tab: up and down, or sideways with Shift
    /// held or on a wheel that tilts. It leaves the selection alone.
    pub(in crate::app) fn scroll_flow(
        &mut self,
        kind: MouseEventKind,
        modifiers: KeyModifiers,
        lines: usize,
    ) {
        let sideways = modifiers.contains(KeyModifiers::SHIFT);
        if self.flow.mode == FlowMode::Timeline {
            let rows = isize::try_from(lines).unwrap_or(isize::MAX);
            let timeline = &mut self.flow.timeline;
            match kind {
                MouseEventKind::ScrollUp if sideways => timeline.pan(-0.1),
                MouseEventKind::ScrollDown if sideways => timeline.pan(0.1),
                MouseEventKind::ScrollUp => timeline.scroll_by(-rows),
                MouseEventKind::ScrollDown => timeline.scroll_by(rows),
                MouseEventKind::ScrollLeft => timeline.pan(-0.1),
                MouseEventKind::ScrollRight => timeline.pan(0.1),
                _ => {}
            }
            return;
        }
        let lines = cells(lines);
        let (dx, dy) = match kind {
            MouseEventKind::ScrollUp if sideways => (-WHEEL_COLUMNS, 0.0),
            MouseEventKind::ScrollDown if sideways => (WHEEL_COLUMNS, 0.0),
            MouseEventKind::ScrollUp => (0.0, -lines),
            MouseEventKind::ScrollDown => (0.0, lines),
            MouseEventKind::ScrollLeft => (-WHEEL_COLUMNS, 0.0),
            MouseEventKind::ScrollRight => (WHEEL_COLUMNS, 0.0),
            _ => return,
        };
        self.flow.pan_by(dx, dy);
    }

    /// A press on the timeline: a row's fold marker folds it, a row or
    /// turn is selected, and a turn already selected opens.
    pub(super) fn click_timeline(&mut self, column: u16, row: u16) -> bool {
        self.flow.ensure_timeline();
        let timeline = &mut self.flow.timeline;
        match timeline.spot_at(column, row) {
            Some(Err(row)) => timeline.toggle(row),
            Some(Ok((row, span))) => {
                let again =
                    span.is_some() && timeline.selected_row() == Some(row) && timeline.span == span;
                if again {
                    self.open_timeline_selection();
                } else {
                    timeline.select_span(row, span);
                }
            }
            None => return false,
        }
        true
    }

    /// A press in the GRAPH tab: on a turn's fold marker it folds the
    /// turn, on a card's line it selects it, and on the selected line it
    /// opens it. On empty canvas it starts a drag that pans the view.
    /// Returns whether the press was on the canvas.
    pub(in crate::app) fn click_flow(&mut self, column: u16, row: u16) -> bool {
        if self.flow.mode == FlowMode::Timeline {
            return self.click_timeline(column, row);
        }
        self.flow.ensure_layout();
        // The map is drawn in cells over a hosted canvas too.
        if let Some(at) = self.flow.minimap.and_then(|map| map.canvas_at(column, row)) {
            self.flow.center_on(at);
            self.flow.minimap_drag = true;
            return true;
        }
        if self.flow.hosted {
            // The front end works its own canvas.
            return false;
        }
        match self.flow.spot_at_cell(column, row) {
            Some(spot) => {
                if self.flow.click(spot) {
                    self.open_flow_node();
                }
                true
            }
            None if self
                .flow
                .view
                .is_some_and(|view| view.canvas_at(column, row).is_some()) =>
            {
                self.flow.drag = Some((column, row));
                true
            }
            None => false,
        }
    }

    /// The pointer moved to `(column, row)` with the button down. Returns
    /// whether a drag of the GRAPH tab's view is under way.
    pub(in crate::app) fn drag_flow(&mut self, column: u16, row: u16) -> bool {
        if self.flow.minimap_drag {
            if let Some(map) = self.flow.minimap {
                self.flow.center_on(map.canvas_near(column, row));
            }
            return true;
        }
        let Some((from_column, from_row)) = self.flow.drag else {
            return false;
        };
        let dx = f32::from(from_column) - f32::from(column);
        let dy = f32::from(from_row) - f32::from(row);
        self.flow.pan_by(dx, dy);
        self.flow.drag = Some((column, row));
        true
    }

    /// The button came up. Returns whether it ended a drag of the GRAPH
    /// tab's view.
    pub(in crate::app) fn release_flow(&mut self) -> bool {
        std::mem::take(&mut self.flow.minimap_drag) | self.flow.drag.take().is_some()
    }
}
