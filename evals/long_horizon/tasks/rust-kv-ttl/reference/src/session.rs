use crate::clock::{Clock, SystemClock};
use crate::command::{execute, parse, Command};
use crate::store::Store;

struct Transaction {
    queued: Vec<Command>,
    failed: bool,
}

pub struct Session<C: Clock = SystemClock> {
    store: Store<C>,
    transaction: Option<Transaction>,
}

impl<C: Clock> Session<C> {
    pub fn new(store: Store<C>) -> Self {
        Self {
            store,
            transaction: None,
        }
    }

    pub fn store(&self) -> &Store<C> {
        &self.store
    }

    pub fn execute_line(&mut self, line: &str) -> String {
        let name = line
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        match (name.as_str(), self.transaction.is_some()) {
            ("MULTI", false) => {
                self.transaction = Some(Transaction {
                    queued: Vec::new(),
                    failed: false,
                });
                return "OK".to_string();
            }
            ("MULTI", true) => return "(error) ERR MULTI calls can not be nested".to_string(),
            ("EXEC", false) => return "(error) ERR EXEC without MULTI".to_string(),
            ("DISCARD", false) => return "(error) ERR DISCARD without MULTI".to_string(),
            ("DISCARD", true) => {
                self.transaction = None;
                return "OK".to_string();
            }
            ("EXEC", true) => return self.exec(),
            _ => {}
        }
        match (parse(line), self.transaction.as_mut()) {
            (Ok(command), Some(tx)) => {
                tx.queued.push(command);
                "QUEUED".to_string()
            }
            (Ok(command), None) => execute(&mut self.store, command).to_string(),
            (Err(e), tx) => {
                if let Some(tx) = tx {
                    tx.failed = true;
                }
                format!("(error) {e}")
            }
        }
    }

    fn exec(&mut self) -> String {
        let Some(tx) = self.transaction.take() else {
            return "(error) ERR EXEC without MULTI".to_string();
        };
        if tx.failed {
            return "(error) EXECABORT Transaction discarded because of previous errors."
                .to_string();
        }
        if tx.queued.is_empty() {
            return "(empty array)".to_string();
        }
        tx.queued
            .into_iter()
            .enumerate()
            .map(|(i, command)| format!("{}) {}", i + 1, execute(&mut self.store, command)))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
