//! The channels between the UI and the kernel.

use crate::events::{UiCommand, UiEvent};
use tokio::sync::mpsc;

/// The event sender and command receiver connecting the UI to the kernel.
#[derive(Debug, Default)]
pub(crate) struct KernelLink {
    /// Event sender (for sending events to kernel)
    pub(crate) event_tx: Option<mpsc::Sender<UiEvent>>,
    /// Command receiver (for receiving commands from kernel)
    pub(crate) command_rx: Option<mpsc::Receiver<UiCommand>>,
}
