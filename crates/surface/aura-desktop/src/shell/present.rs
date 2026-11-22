//! Frames shown on the GPU from a thread of their own.
//!
//! Showing a frame waits on the display: for the frame before it to be
//! taken, up to a refresh with vsync, and as long as the compositor
//! likes while the window is hidden. The window's thread hands each
//! frame over here and goes back to input and commands. It is told when
//! the frame was shown ([`Wake::Presented`]) and asks for the next only
//! then, so frames never queue up behind each other.

use super::screen::Showing;
use super::window::Wake;
use crate::gpu::Gpu;
use crate::scene::Scene;
use crate::text::{Atlas, AtlasRows};
use anyhow::Context as _;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use winit::window::Window;

/// How long closing waits for a frame being shown.
const CLOSING: Duration = Duration::from_millis(500);

/// A frame handed over: its quads, and the atlas rows the GPU lacks for
/// them.
struct Frame {
    scene: Scene,
    atlas: AtlasRows,
}

#[derive(Default)]
struct Slot {
    /// The frame to show next
    frame: Option<Frame>,
    /// A scene shown already, whose room the next frame is copied into
    spare: Option<Scene>,
    closing: bool,
}

#[derive(Default)]
struct Mailbox {
    slot: Mutex<Slot>,
    filled: Condvar,
}

impl Mailbox {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The thread showing frames, as the window's thread sees it.
pub struct Presenter {
    mailbox: Arc<Mailbox>,
    thread: Option<JoinHandle<()>>,
    /// Disconnects when the thread ends
    ended: mpsc::Receiver<()>,
    /// Frames handed over that the thread has not reported
    pending: usize,
    /// The atlas's size in the last frame handed over
    sent: Option<(u32, u32)>,
    atlas_limit: u32,
}

impl Presenter {
    /// Show frames on `window` with `gpu` from a new thread, which tells
    /// the loop with `wake` how each went.
    ///
    /// # Errors
    ///
    /// Fails when the thread can't be started.
    pub fn start(
        gpu: Gpu,
        window: Arc<Window>,
        wake: impl Fn(Wake) -> bool + Send + 'static,
    ) -> anyhow::Result<Self> {
        let atlas_limit = gpu.atlas_limit();
        let mailbox = Arc::new(Mailbox::default());
        let (end, ended) = mpsc::channel::<()>();
        let shared = Arc::clone(&mailbox);
        let thread = std::thread::Builder::new()
            .name("aura-present".to_string())
            .spawn(move || {
                let _end = end;
                show(gpu, &shared, &window, &wake);
            })
            .context("starting the thread that shows frames")?;
        Ok(Self {
            mailbox,
            thread: Some(thread),
            ended,
            pending: 0,
            sent: None,
            atlas_limit,
        })
    }

    /// The tallest the atlas may grow ([`Atlas::set_limit`]).
    pub const fn atlas_limit(&self) -> u32 {
        self.atlas_limit
    }

    /// Whether every frame handed over was reported, so another can be
    /// drawn without waiting behind it.
    pub const fn ready(&self) -> bool {
        self.pending == 0
    }

    /// Hand `scene` over with the rows of `atlas` the GPU lacks for it,
    /// in place of a frame handed over before that the thread has not
    /// taken.
    pub fn hand_over(&mut self, scene: &Scene, atlas: &mut Atlas) {
        let older = self.mailbox.lock().frame.take();
        let mut rows = atlas.take_rows(self.sent);
        let mut copy = match older {
            // Never shown, so never reported: the new frame is reported
            // in its place.
            Some(older) => {
                rows = atlas.merge_rows(rows, &older.atlas);
                older.scene
            }
            None => {
                self.pending += 1;
                self.mailbox.lock().spare.take().unwrap_or_default()
            }
        };
        self.sent = Some(rows.size);
        copy.clone_from(scene);
        self.mailbox.lock().frame = Some(Frame {
            scene: copy,
            atlas: rows,
        });
        self.mailbox.filled.notify_one();
    }

    /// The thread reported a frame.
    pub fn presented(&mut self) {
        self.pending = self.pending.saturating_sub(1);
    }
}

impl Drop for Presenter {
    /// Stop the thread, and with it the GPU, which must go before the
    /// window does. A thread stuck showing a frame is left behind rather
    /// than holding the window open.
    fn drop(&mut self) {
        self.mailbox.lock().closing = true;
        self.mailbox.filled.notify_one();
        match self.ended.recv_timeout(CLOSING) {
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
            }
            Ok(()) | Err(RecvTimeoutError::Timeout) => {
                tracing::warn!("the thread showing frames did not stop");
            }
        }
    }
}

/// Show the frames put in `mailbox` on `window` until it closes or
/// showing fails, saying how each went with `wake`.
fn show(mut gpu: Gpu, mailbox: &Mailbox, window: &Window, wake: &impl Fn(Wake) -> bool) {
    loop {
        let frame = {
            let mut slot = mailbox.lock();
            loop {
                if slot.closing {
                    return;
                }
                if let Some(frame) = slot.frame.take() {
                    break frame;
                }
                slot = mailbox
                    .filled
                    .wait(slot)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let started = Instant::now();
        let showing = gpu
            .render(&frame.scene, &frame.atlas, || window.pre_present_notify())
            .map(|shown| {
                if shown {
                    Showing::Done(started.elapsed())
                } else {
                    Showing::Missed
                }
            });
        let failed = showing.is_err();
        mailbox.lock().spare = Some(frame.scene);
        if !wake(Wake::Presented(showing)) || failed {
            return;
        }
    }
}
