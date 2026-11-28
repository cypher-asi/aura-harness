//! The terminal UI's selection, kept between runs in the session's data
//! directory (see [`aura_ui_terminal::LastSelection`]). The file is read
//! and written on the blocking pool, never on an async worker thread.

use aura_ui_terminal::LastSelection;
use std::path::PathBuf;
use tracing::warn;

/// The selection saved at `path`; the default when there is none or it
/// can't be read.
pub(crate) async fn load(path: PathBuf) -> LastSelection {
    tokio::task::spawn_blocking(move || LastSelection::load(&path))
        .await
        .unwrap_or_default()
}

/// Save `selection` at `path`. A failure is logged: the next run then
/// opens on its usual first screen, which is no reason to fail this one.
pub(crate) async fn save(path: PathBuf, selection: LastSelection) {
    let shown = path.display().to_string();
    match tokio::task::spawn_blocking(move || selection.save(&path)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!(error = %e, path = %shown, "could not save the last selection"),
        Err(e) => warn!(error = %e, path = %shown, "saving the last selection did not finish"),
    }
}
