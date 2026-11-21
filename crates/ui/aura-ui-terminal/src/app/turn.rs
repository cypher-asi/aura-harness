//! The agent's turn: where it starts in the chat, and the kernel records it
//! wrote.

use super::{App, NotificationType, PanelFocus};
use crate::components::Message;
use crate::events::MessageRole;

/// Records stamped up to this long before a prompt still count as its
/// turn's, as the kernel and the chat read the clock apart.
const RECORD_CLOCK_SLACK_MS: u64 = 1000;

impl App {
    /// When the latest prompt was sent, in Unix milliseconds.
    #[must_use]
    pub(crate) fn turn_started_ms(&self) -> Option<u64> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role() == MessageRole::User && m.shell_run().is_none())
            .map(Message::timestamp_ms)
    }

    /// Highlight the first kernel record of the turn whose prompt was sent
    /// at `started_ms`, with the records panel focused so the arrow keys
    /// step through the turn's records in order.
    pub(super) fn show_turn_records(&mut self, started_ms: u64) {
        let since = started_ms.saturating_sub(RECORD_CLOCK_SLACK_MS);
        let Some(index) = self.records.iter().position(|r| r.ts_ms >= since) else {
            self.notification = Some((
                "No kernel records for this turn yet".to_string(),
                NotificationType::Warning,
            ));
            return;
        };
        self.selected_record = index;
        self.records_tab = super::RecordsTab::Kernel;
        self.focus = PanelFocus::Records;
    }
}
