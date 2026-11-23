//! Gate's latest routing decisions in this session.

use super::GateView;
use crate::record_loader::format_local_time;
use std::fmt::Write;

impl GateView<'_> {
    /// Gate's latest decisions, newest last, or that it made none yet.
    pub(super) fn render_recent(&self, out: &mut String) {
        if self.recent.is_empty() {
            out.push_str("\n\nNo routing decisions yet in this session.");
            return;
        }
        let _ = write!(out, "\n\nLast {} decisions, newest last", self.recent.len());
        for route in self.recent {
            let _ = write!(
                out,
                "\n  {}  {:<7}  {:<17}  {}  {}",
                format_local_time(route.ts_ms),
                route.level.as_str(),
                route.category.as_str(),
                route.model,
                route.reason,
            );
        }
    }
}
