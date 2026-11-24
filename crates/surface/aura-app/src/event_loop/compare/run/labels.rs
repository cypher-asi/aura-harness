//! The classifier's label of each prompt a comparison's runs send, asked
//! for once and shared: every run of either arm that sends the same
//! prompt works from the same label, so a classifier that labels the same
//! text differently from one call to the next does not set the arms'
//! work apart.

use aura_gate::judge::classify::Label;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::OnceCell;

/// The labels of a comparison's prompts, by prompt text.
#[derive(Default)]
pub(super) struct Labels {
    by_text: Mutex<HashMap<String, Arc<OnceCell<Label>>>>,
}

impl Labels {
    /// The label of `text`: the one a run already got, else what
    /// `classify` gives. Runs that ask while it classifies wait for it.
    /// When it fails, the next run to ask classifies again.
    ///
    /// # Errors
    /// Says why when `classify` fails.
    pub(super) async fn of<F, Fut>(&self, text: &str, classify: F) -> Result<Label, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Label, String>>,
    {
        let cell = {
            let mut by_text = self
                .by_text
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(by_text.entry(text.to_string()).or_default())
        };
        cell.get_or_try_init(classify).await.cloned()
    }
}
