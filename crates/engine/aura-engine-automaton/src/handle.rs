use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::types::{AutomatonId, AutomatonStatus};

/// Client-side handle for controlling a running [`crate::runtime::Automaton`] (stop, pause, resume, status).
pub struct AutomatonHandle {
    id: AutomatonId,
    cancel: CancellationToken,
    status_rx: watch::Receiver<AutomatonStatus>,
    pause_tx: watch::Sender<bool>,
}

impl AutomatonHandle {
    pub(crate) fn new(
        id: AutomatonId,
        cancel: CancellationToken,
        status_rx: watch::Receiver<AutomatonStatus>,
        pause_tx: watch::Sender<bool>,
    ) -> Self {
        Self {
            id,
            cancel,
            status_rx,
            pause_tx,
        }
    }

    pub fn id(&self) -> &AutomatonId {
        &self.id
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }

    pub fn pause(&self) {
        let _ = self.pause_tx.send(true);
    }

    pub fn resume(&self) {
        let _ = self.pause_tx.send(false);
    }

    pub fn status(&self) -> AutomatonStatus {
        *self.status_rx.borrow()
    }

    pub fn is_finished(&self) -> bool {
        matches!(
            self.status(),
            AutomatonStatus::Stopped | AutomatonStatus::Failed | AutomatonStatus::Completed
        )
    }

    pub async fn wait(&mut self) {
        while !self.is_finished() {
            if self.status_rx.changed().await.is_err() {
                break;
            }
        }
    }
}
