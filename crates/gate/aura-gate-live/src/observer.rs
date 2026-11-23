//! The table kept current as every kernel's record observer: each prompt
//! and `Outcome` is taken in as it is committed, and what the user says
//! of an attempt goes to the attempt log.

use crate::table::{lock, GateLive};
use aura_agent_kernel::RecordObserver;
use aura_core_types::{RecordEntry, TransactionType};

impl RecordObserver for GateLive {
    fn on_entry(&self, entry: &RecordEntry) {
        if matches!(
            entry.tx.tx_type,
            TransactionType::UserPrompt | TransactionType::Outcome
        ) {
            lock(&self.stats).observe(entry);
            self.log_user_signal(entry);
        }
    }
}
