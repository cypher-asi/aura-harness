//! Keys of the GRAPH tab's canvas and timeline.

use super::layout::CardKind;
use super::{timeline, FlowMode, FlowState, ZOOM_STEP};
use crate::app::{App, ChatTab, NotificationType};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    /// Keys of the GRAPH tab, used while the input line is empty. Returns
    /// whether the key was used.
    pub(in crate::app) fn handle_flow_key(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        if self.flow.mode == FlowMode::Timeline {
            return self.handle_timeline_key(key);
        }
        self.flow.ensure_layout();
        self.flow.handle_move_key(key.code) || self.handle_flow_action_key(key.code)
    }

    /// Keys of the GRAPH tab's canvas that fold, open, jump or zoom.
    fn handle_flow_action_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Esc if self.flow.from_timeline => self.show_chat_tab(ChatTab::Time),
            KeyCode::Esc => self.show_chat_tab(ChatTab::Chat),
            KeyCode::Char('m') => self.flow.toggle_minimap(),
            KeyCode::Char('c') => self.flow.toggle_selected_lane(),
            KeyCode::Char('C') => self.flow.fold_other_lanes(),
            KeyCode::Char('x') => self.flow.unfold_lanes(),
            KeyCode::Enter if self.flow.agent_card_selected() => self.flow.toggle_selected_card(),
            KeyCode::Char(' ') => self.flow.toggle_fold(),
            KeyCode::Char('z') => self.flow.toggle_fold_all(),
            KeyCode::Enter => self.open_flow_node(),
            KeyCode::Char('f') => self.follow_flow_link(),
            KeyCode::Char('r') => self.jump_to_flow_record(),
            KeyCode::Char('o') => self.open_flow_agent(),
            KeyCode::Char('+' | '=') if self.host_canvas => self.zoom_flow(ZOOM_STEP, None),
            KeyCode::Char('-') if self.host_canvas => self.zoom_flow(1.0 / ZOOM_STEP, None),
            KeyCode::Char('0') => self.reset_flow_zoom(),
            _ => return false,
        }
        true
    }

    /// `f`: jump along a link of the selected node, or say it has none.
    fn follow_flow_link(&mut self) {
        if !self.flow.follow_link() {
            self.notifications.current = Some((
                "This step has no link to another step".to_string(),
                NotificationType::Warning,
            ));
        }
    }

    /// `0`: back to drawing a canvas cell to a screen cell, on the selection.
    fn reset_flow_zoom(&mut self) {
        let back = 1.0 / self.flow.zoom();
        self.zoom_flow(back, None);
        self.flow.detached = false;
    }

    /// Keys of the GRAPH tab's timeline.
    pub(super) fn handle_timeline_key(&mut self, key: KeyEvent) -> bool {
        self.flow.ensure_timeline();
        let timeline = &mut self.flow.timeline;
        let page = timeline.view.map_or(10, |view| {
            isize::try_from(view.rows_shown.max(1)).unwrap_or(10)
        });
        let last = timeline.timeline.rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => self.show_chat_tab(ChatTab::Chat),
            KeyCode::Up | KeyCode::Char('k') => timeline.move_row(-1),
            KeyCode::Down | KeyCode::Char('j') => timeline.move_row(1),
            KeyCode::Left | KeyCode::Char('h') => timeline.move_span(false),
            KeyCode::Right | KeyCode::Char('l') => timeline.move_span(true),
            KeyCode::PageUp => timeline.scroll_by(-page),
            KeyCode::PageDown => timeline.scroll_by(page),
            KeyCode::Home | KeyCode::Char('g') => timeline.select_row(0),
            KeyCode::End | KeyCode::Char('G') => timeline.select_row(last),
            KeyCode::Char(' ') => {
                if let Some(row) = timeline.selected_row() {
                    timeline.toggle(row);
                }
            }
            KeyCode::Char('z') => timeline.toggle_all(),
            KeyCode::Char('e') => timeline.toggle_issues(),
            KeyCode::Char('+' | '=') => timeline.zoom_by(Some(timeline::ZOOM_STEP)),
            KeyCode::Char('-') => timeline.zoom_by(Some(1.0 / timeline::ZOOM_STEP)),
            KeyCode::Char('0') => timeline.zoom_by(None),
            KeyCode::Enter => self.open_timeline_selection(),
            KeyCode::Char('o') => {
                let lane = timeline
                    .selected_row()
                    .and_then(|row| timeline.timeline.rows[row].lane);
                if let Some(lane) = lane {
                    let agent = self.flow.lanes[lane].agent_id.clone();
                    self.open_agent(agent);
                }
            }
            _ => return false,
        }
        true
    }

    /// Enter on the timeline: the cards of the selected turn, else of the
    /// row's first; a row without turns folds.
    pub(super) fn open_timeline_selection(&mut self) {
        let timeline = &self.flow.timeline;
        let Some(row) = timeline.selected_row() else {
            return;
        };
        let it = &timeline.timeline.rows[row];
        let head = timeline
            .selected_span()
            .map(|(row, span)| timeline.timeline.rows[row].spans[span].head)
            .or_else(|| it.spans.first().map(|span| span.head));
        match head {
            Some(head) if it.lane.is_some() => self.flow.drill_in(head),
            _ => self.flow.timeline.toggle(row),
        }
    }
}

impl FlowState {
    /// Keys of the GRAPH tab's canvas that move the selection or the view.
    fn handle_move_key(&mut self, code: KeyCode) -> bool {
        let page = self.view.map_or(1.0, |view| {
            f32::from(view.area.height.saturating_sub(2).max(1)) / self.zoom()
        });
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_line(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_line(true),
            KeyCode::Left | KeyCode::Char('h') => self.move_along(false),
            KeyCode::Right | KeyCode::Char('l') => self.move_along(true),
            KeyCode::PageUp => self.pan_by(0.0, -page),
            KeyCode::PageDown => self.pan_by(0.0, page),
            KeyCode::Home | KeyCode::Char('g') => self.select_first(),
            KeyCode::End | KeyCode::Char('G') => self.select_newest(),
            KeyCode::Char('[') => self.move_to_prompt(false),
            KeyCode::Char(']') => self.move_to_prompt(true),
            _ => return false,
        }
        true
    }

    /// `m`: hide the map of the whole canvas, or show it again.
    fn toggle_minimap(&mut self) {
        self.minimap_hidden = !self.minimap_hidden;
        self.minimap_drag = false;
    }

    /// `c`: show the selected node's agent as one card, or its turns again.
    fn toggle_selected_lane(&mut self) {
        if let Some(node) = self.selected_node_index() {
            self.toggle_lane_of(node);
        }
    }

    fn agent_card_selected(&self) -> bool {
        self.selected_card()
            .is_some_and(|card| self.layout.cards[card].kind == CardKind::Agent)
    }

    fn toggle_selected_card(&mut self) {
        if let Some(card) = self.selected_card() {
            self.toggle_card(card);
        }
    }
}
