//! What Gate needs of the running app: threads startup doesn't wait for.

/// Run `work` on a blocking thread of the current Tokio runtime, or here
/// when there is none.
pub(crate) fn in_background(work: impl FnOnce() + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn_blocking(work);
        }
        Err(_) => work(),
    }
}
