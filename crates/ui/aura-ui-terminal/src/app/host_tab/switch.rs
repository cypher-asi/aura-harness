//! The footer's placement switch: its label and button, a click and the
//! pointer on it, and the sweep across its label when a move lands.

use super::App;
use crate::animation::shimmer_pass_frames;
use crate::app::{contains, HoverTarget, NotificationType};
use crate::events::UiEvent;
use crate::host::{PlacementState, PlacementView};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

/// Columns kept between the status and the switch.
pub const SWITCH_GAP: u16 = 2;

/// Columns either side of the label that light with it under the pointer
/// and take its clicks: the left one from the gap before it, the right
/// one from the footer's edge inset.
const SWITCH_PAD: u16 = 1;

/// A move that landed on the other placement.
#[derive(Debug)]
pub(super) struct Landing {
    /// The agent moved, by hex id
    agent_id: String,
    /// The animation frame it landed on
    frame: usize,
    /// Frames the sweep across its new label takes
    frames: usize,
}

/// Who placed a cloud agent, when it is not the user signed in here.
fn foreign_owner(state: &PlacementState) -> Option<&str> {
    state
        .owner
        .as_deref()
        .filter(|_| state.placement == PlacementView::Cloud)
}

/// The switch's text for `state`.
#[must_use]
pub fn switch_label(state: &PlacementState) -> String {
    if let Some(moving) = &state.moving {
        return format!("◐ {moving}");
    }
    if let Some(owner) = foreign_owner(state) {
        return format!("{} {owner}", PlacementView::Cloud.mark());
    }
    let mut label = format!("{} {}", state.placement.mark(), state.placement.label());
    if let Some(machine) = state
        .machine_state
        .as_deref()
        .filter(|_| state.placement == PlacementView::Cloud)
    {
        if machine != "ready" {
            label.push_str(" · ");
            label.push_str(machine);
        }
    }
    label
}

/// Where the switch goes in the footer `row`: its right end.
#[must_use]
pub fn switch_rect(row: Rect, label: &str) -> Option<Rect> {
    let width = u16::try_from(unicode_width::UnicodeWidthStr::width(label)).unwrap_or(u16::MAX);
    if row.width < width + SWITCH_GAP + 8 || row.height == 0 {
        return None;
    }
    Some(Rect {
        x: row.right() - width,
        width,
        height: 1,
        ..row
    })
}

/// The button around the switch drawn at `rect`: its cells and a column
/// either side.
#[must_use]
pub(crate) fn switch_button(rect: Rect) -> Rect {
    let x = rect.x.saturating_sub(SWITCH_PAD);
    Rect {
        x,
        width: rect.right().saturating_add(SWITCH_PAD) - x,
        ..rect
    }
}

impl App {
    /// Note the footer row drawn this frame, for the pointer on the
    /// switch.
    pub fn set_footer_area(&mut self, area: Rect) {
        let row = Rect { height: 1, ..area };
        self.host.cloud.switch =
            switch_rect(row, &switch_label(&self.placement())).map(switch_button);
    }

    /// Whether `column`, `row` is on the switch's button as last drawn.
    pub(in crate::app) fn on_placement_switch(&self, column: u16, row: u16) -> bool {
        contains(self.host.cloud.switch, column, row)
    }

    /// Whether the switch is lit under the pointer. It isn't while it
    /// shows a move, nor on another user's agent, where a click only
    /// says why nothing happens.
    #[must_use]
    pub(crate) fn placement_switch_lit(&self) -> bool {
        self.hover() == Some(HoverTarget::PlacementSwitch) && {
            let state = self.placement();
            state.moving.is_none() && foreign_owner(&state).is_none()
        }
    }

    /// Ask for the agent on screen to run at `placement`, and show the
    /// move on the switch at once.
    pub(super) fn request_placement(&mut self, placement: PlacementView) {
        let current = self.placement();
        if let Some(owner) = foreign_owner(&current) {
            let note = format!("Placed by {owner}; sign in as {owner} to manage it");
            self.notify(&note, NotificationType::Warning);
            return;
        }
        if current.moving.is_some() {
            self.notify("A move is already under way", NotificationType::Success);
            return;
        }
        if current.placement == placement {
            let note = format!(
                "This agent already runs {}",
                match placement {
                    PlacementView::Local => "on this machine",
                    PlacementView::Cloud => "in the cloud",
                }
            );
            self.notify(&note, NotificationType::Success);
            return;
        }
        if self.is_busy() {
            self.notify(
                "The agent is working; stop it or wait before moving it",
                NotificationType::Warning,
            );
            return;
        }
        let asked = self.try_send_event(UiEvent::SetPlacement {
            agent_id: self.active_agent_id.clone(),
            placement,
        });
        if asked {
            // A guess until the app's next update, which confirms it or
            // puts the switch back.
            let guess = PlacementState {
                moving: Some(current.placement.moving_label().to_string()),
                ..current
            };
            let agent_id = self.active_agent_id.clone();
            self.host.cloud.placements.insert(agent_id, guess);
        }
    }

    /// A left press on the switch flips the agent's placement. Returns
    /// whether the press was on it.
    pub(in crate::app) fn click_placement_switch(&mut self, mouse: MouseEvent) -> bool {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left)
            || !self.on_placement_switch(mouse.column, mouse.row)
        {
            return false;
        }
        self.request_placement(self.placement().placement.flipped());
        true
    }

    /// Start the sweep across the switch's label when `state`, the next
    /// placement of `agent_id`, ends its move on the other placement.
    pub(super) fn note_landing(&mut self, agent_id: &str, state: &PlacementState) {
        let landed = agent_id == self.active_agent_id
            && state.moving.is_none()
            && self
                .host
                .cloud
                .placements
                .get(agent_id)
                .is_some_and(|old| old.moving.is_some() && old.placement != state.placement);
        if landed {
            self.host.cloud.landed = Some(Landing {
                agent_id: agent_id.to_string(),
                frame: self.ticks.animation_frame,
                frames: shimmer_pass_frames(&switch_label(state)),
            });
        }
    }

    /// How many frames into the sweep across the switch's new label this
    /// one is, while it runs.
    #[must_use]
    pub(crate) fn switch_sweep(&self) -> Option<usize> {
        let landing = self.host.cloud.landed.as_ref()?;
        let step = self.ticks.animation_frame.wrapping_sub(landing.frame);
        (landing.agent_id == self.active_agent_id && step < landing.frames).then_some(step)
    }

    /// Whether the switch changes from frame to frame: the agent on screen
    /// is moving, or its new label is being swept.
    pub(in crate::app) fn placement_switch_animating(&self) -> bool {
        self.host
            .cloud
            .placements
            .get(&self.active_agent_id)
            .is_some_and(|state| state.moving.is_some())
            || self.switch_sweep().is_some()
    }
}
