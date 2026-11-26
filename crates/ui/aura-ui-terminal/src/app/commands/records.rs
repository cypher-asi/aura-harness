//! The Record panel: the active agent's records, oldest first.

use crate::app::{App, DetailView, MAX_RECORDS};
use crate::events::{RecordSummary, UiCommand};

impl App {
    /// Apply a command about the records; any other comes back.
    pub(super) fn record_ui_command(&mut self, cmd: UiCommand) -> Option<UiCommand> {
        match cmd {
            UiCommand::NewRecord(record) => self.cmd_new_record(record),
            UiCommand::ClearRecords => self.cmd_clear_records(),
            UiCommand::SetRecords(records) => self.cmd_set_records(records),
            other => return Some(other),
        }
        None
    }

    /// Append a record, oldest first. A selection on the newest record
    /// follows new arrivals; one moved up stays on its record.
    fn cmd_new_record(&mut self, record: RecordSummary) {
        // Agents running in the background write records too; the panel
        // lists the active agent's.
        if !record.agent_id.is_empty()
            && !self.active_agent_id.is_empty()
            && record.agent_id != self.active_agent_id
        {
            return;
        }
        let follow = self.detail.view != Some(DetailView::Record)
            && self.records.selected + 1 >= self.records.items.len();
        self.records.items.push_back(record);
        while self.records.items.len() > MAX_RECORDS {
            self.records.items.pop_front();
            self.records.selected = self.records.selected.saturating_sub(1);
        }
        if follow {
            self.records.selected = self.records.items.len() - 1;
        }
    }

    fn cmd_clear_records(&mut self) {
        self.records.items.clear();
        self.records.selected = 0;
        if self.detail.view == Some(DetailView::Record) {
            self.close_detail();
        }
    }

    /// Replace the records with `records`, oldest first, keeping the
    /// newest [`MAX_RECORDS`]. The records held already, as when an
    /// agent comes back on screen, stay as they are, highlight, scroll and
    /// Record detail view included. Otherwise the detail view closes, and
    /// the highlight stays on its record, or follows the newest when it
    /// was on it. Records all of another agent, sent for an agent the
    /// screen has left since, are dropped. The agent's records newer than
    /// all of `records` stay after them: written after `records` were read,
    /// they came on their own before them.
    fn cmd_set_records(&mut self, records: Vec<RecordSummary>) {
        let sent = records.len();
        let active = &self.active_agent_id;
        let mut kept: std::collections::VecDeque<_> = records
            .into_iter()
            .filter(|record| {
                record.agent_id.is_empty() || active.is_empty() || record.agent_id == *active
            })
            .collect();
        if sent > 0 && kept.is_empty() {
            return;
        }
        let read_up_to = kept.back().map_or(0, |record| record.seq);
        kept.extend(
            self.records
                .items
                .iter()
                .filter(|record| {
                    !active.is_empty() && record.agent_id == *active && record.seq > read_up_to
                })
                .cloned(),
        );
        let excess = kept.len().saturating_sub(MAX_RECORDS);
        kept.drain(..excess);
        let same =
            |a: &RecordSummary, b: &RecordSummary| a.seq == b.seq && a.full_hash == b.full_hash;
        if kept.len() == self.records.items.len()
            && kept
                .iter()
                .zip(&self.records.items)
                .all(|(a, b)| same(a, b))
        {
            return;
        }
        if self.detail.view == Some(DetailView::Record) {
            self.close_detail();
        }
        let newest = self.records.selected + 1 >= self.records.items.len();
        let on = self.records.items.get(self.records.selected).map(|r| r.seq);
        self.records.items = kept;
        let last = self.records.items.len().saturating_sub(1);
        self.records.selected = on
            .filter(|_| !newest)
            .and_then(|seq| self.records.items.iter().position(|r| r.seq == seq))
            .unwrap_or(last);
    }
}
