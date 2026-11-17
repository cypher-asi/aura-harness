//! The shell in a window: the event loop, and what only a window can do.
//!
//! The loop sleeps until the window is told something, the session sends
//! commands, or the shell's next deadline comes. Whatever happened is
//! handed to the [`Shell`] at once; when that leaves it looking
//! different a frame is asked for, and the display hands those out one
//! per refresh. So a hundred commands and ten keys between two refreshes
//! cost one frame, and a window nothing happens in draws nothing.
//!
//! Commands come from the session's thread. They are left in an inbox
//! and the loop is woken once for however many are waiting.

use super::keys::Press;
use super::{Effect, Shell};
use crate::app::{Handles, Options, MAX_BATCH, WINDOW};
use crate::geom::{Point, Size};
use crate::gpu::Gpu;
use crate::scene::Scene;
use crate::text;
use anyhow::Context;
use aura_ui::events::UiCommand;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;
use tokio::sync::{mpsc, Notify};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::SmolStr;
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{CursorIcon, UserAttentionType, Window, WindowId};

/// Commands the inbox holds before the session is made to wait for the
/// window to take them.
const INBOX_ROOM: usize = 4 * MAX_BATCH;

/// The tallest the glyph atlas is let grow.
const ATLAS_LIMIT: u32 = 4096;

/// Why the loop was woken from outside the window.
pub enum Wake {
    /// Commands are waiting in the inbox.
    Commands,
    /// The session closed its end of the command channel.
    SessionEnded,
    /// The editor a tab (by id) opened on a file exited.
    EditorClosed {
        tab: u64,
        path: PathBuf,
        result: Result<(), String>,
    },
}

/// Commands from the session that the window has not taken yet.
#[derive(Default)]
struct Inbox {
    queue: Mutex<Vec<UiCommand>>,
    /// Whether the loop was woken for the queue and has not taken it
    waking: AtomicBool,
    /// Told when the queue is taken
    room: Notify,
}

impl Inbox {
    /// Everything waiting.
    fn take(&self) -> Vec<UiCommand> {
        // Before the queue is emptied: a command put in after that wakes
        // the loop again.
        self.waking.store(false, Ordering::Release);
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let batch = std::mem::take(&mut *queue);
        drop(queue);
        self.room.notify_one();
        batch
    }
}

/// Move the session's commands into `inbox` as they come, waking the
/// loop with `wake` when it isn't already awake for them. Ends when the
/// session does, or when `wake` says the loop is gone.
async fn pump(
    mut commands: mpsc::Receiver<UiCommand>,
    inbox: Arc<Inbox>,
    wake: impl Fn(Wake) -> bool,
) {
    let mut batch = Vec::new();
    loop {
        if commands.recv_many(&mut batch, MAX_BATCH).await == 0 {
            wake(Wake::SessionEnded);
            return;
        }
        loop {
            {
                let mut queue = inbox.queue.lock().unwrap_or_else(PoisonError::into_inner);
                if queue.len() < INBOX_ROOM {
                    queue.append(&mut batch);
                    break;
                }
            }
            // The window is behind. Leave the rest in the channel, which
            // holds the session up when it fills, until this is taken.
            inbox.room.notified().await;
        }
        if !inbox.waking.swap(true, Ordering::AcqRel) && !wake(Wake::Commands) {
            return;
        }
    }
}

struct Handler {
    shell: Shell,
    inbox: Arc<Inbox>,
    proxy: EventLoopProxy<Wake>,
    runtime: tokio::runtime::Handle,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    clipboard: Option<arboard::Clipboard>,
    /// The frame on screen
    shown: Scene,
    /// The pointer's shape as the window was last told
    icon: CursorIcon,
    /// Whether a frame was asked for and has not come
    asked: bool,
    /// Whether the next frame is shown even when it is the one on screen
    /// again: the window's pixels are gone, or never were
    stale: bool,
    failure: Option<anyhow::Error>,
}

/// Open the window on the session behind `handles` and run it until it
/// closes. Must run on the main thread, inside the async runtime.
///
/// # Errors
///
/// Fails when the window can't be opened or drawn into.
pub fn run(handles: Handles, options: &Options, launched: Instant) -> anyhow::Result<()> {
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .context("starting the event loop")?;
    let runtime = tokio::runtime::Handle::try_current().context("no async runtime")?;
    let proxy = event_loop.create_proxy();
    let inbox = Arc::new(Inbox::default());
    let waker = proxy.clone();
    runtime.spawn(pump(handles.commands, Arc::clone(&inbox), move |wake| {
        waker.send_event(wake).is_ok()
    }));
    let mut handler = Handler {
        shell: Shell::new(handles.events, options, launched),
        inbox,
        proxy,
        runtime,
        window: None,
        gpu: None,
        clipboard: None,
        shown: Scene::default(),
        icon: CursorIcon::Default,
        asked: false,
        stale: true,
        failure: None,
    };
    event_loop
        .run_app(&mut handler)
        .context("running the event loop")?;
    handler.failure.map_or(Ok(()), Err)
}

impl Handler {
    fn open(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<()> {
        let attributes = Window::default_attributes()
            .with_title("AURA")
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(WINDOW.width, WINDOW.height));
        let window = event_loop
            .create_window(attributes)
            .context("opening the window")?;
        let window = Arc::new(window);
        let size = window.inner_size();
        let gpu = Gpu::new(Arc::clone(&window), size.width, size.height)?;
        tracing::info!(adapter = gpu.name(), "drawing on the GPU");
        let limit = gpu.atlas_limit().min(ATLAS_LIMIT);
        text::with(|text| text.atlas_mut().set_limit(limit));
        self.window = Some(window);
        self.gpu = Some(gpu);
        self.fit();
        Ok(())
    }

    /// Take the window's size and the display's scale as they are now.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn fit(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            // Minimized on some systems: nothing to draw into.
            return;
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.resize(size.width, size.height);
        }
        self.shell.resized(
            Size::new(size.width as f32, size.height as f32),
            window.scale_factor() as f32,
        );
        self.stale = true;
    }

    /// Draw a frame, and show it unless it is the one on screen.
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let asked = std::mem::take(&mut self.asked);
        let (Some(window), Some(gpu)) = (&self.window, &mut self.gpu) else {
            return;
        };
        let scene = self.shell.frame(Instant::now());
        // A frame nobody here asked for is the system's: the window was
        // uncovered, and wants its pixels whatever they are.
        let same = asked && !self.stale && *scene == self.shown;
        if same {
            self.shell.stats().shown(None);
            return;
        }
        self.shown.clone_from(scene);
        let started = Instant::now();
        let drawn = text::with(|text| {
            gpu.render(&self.shown, text.atlas_mut(), || {
                window.pre_present_notify();
            })
        });
        match drawn {
            Ok(true) => {
                self.stale = false;
                self.shell.stats().shown(Some(started.elapsed()));
            }
            // The window gave no frame to draw into. The next thing
            // that changes how it looks tries again.
            Ok(false) => self.stale = true,
            Err(error) => {
                self.failure = Some(error);
                event_loop.exit();
            }
        }
    }

    /// Do what the shell asked of its window.
    fn act(&mut self, event_loop: &ActiveEventLoop) {
        for effect in self.shell.take_effects() {
            match effect {
                Effect::Exit => event_loop.exit(),
                Effect::Attention => self.on_window(|window| {
                    window.request_user_attention(Some(UserAttentionType::Informational));
                }),
                Effect::Copy(text) => {
                    let chars = text.chars().count();
                    let result = self.copy(text);
                    self.shell.copied(chars, result);
                }
                Effect::Edit { tab, editor, path } => self.edit(tab, &editor, path),
                Effect::Drag => self.on_window(|window| {
                    let _ = window.drag_window();
                }),
                Effect::Resize(direction) => self.on_window(|window| {
                    let _ = window.drag_resize_window(direction);
                }),
                Effect::Minimize => self.on_window(|window| window.set_minimized(true)),
                Effect::ToggleMaximize => {
                    self.on_window(|window| window.set_maximized(!window.is_maximized()));
                }
            }
        }
    }

    fn on_window(&self, act: impl FnOnce(&Window)) {
        if let Some(window) = &self.window {
            act(window);
        }
    }

    fn copy(&mut self, text: String) -> Result<(), String> {
        if self.clipboard.is_none() {
            // Kept for as long as the window: on some systems the text is
            // only there while its owner is.
            self.clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        let clipboard = self.clipboard.as_mut().ok_or("no clipboard")?;
        clipboard.set_text(text).map_err(|e| e.to_string())
    }

    /// Run `editor` on `path` for a tab, and wake the loop when it exits.
    fn edit(&self, tab: u64, editor: &str, path: PathBuf) {
        let mut words = editor.split_whitespace().map(str::to_string);
        let program = words.next().unwrap_or_default();
        let arguments: Vec<String> = words.collect();
        let proxy = self.proxy.clone();
        self.runtime.spawn(async move {
            let status = tokio::process::Command::new(program)
                .args(arguments)
                .arg(&path)
                .status()
                .await;
            let result = match status {
                Ok(status) if status.success() => Ok(()),
                Ok(status) => Err(format!("editor exited with {status}")),
                Err(e) => Err(e.to_string()),
            };
            let _ = proxy.send_event(Wake::EditorClosed { tab, path, result });
        });
    }
}

impl ApplicationHandler<Wake> for Handler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        if let Err(error) = self.open(event_loop) {
            self.failure = Some(error);
            event_loop.exit();
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, wake: Wake) {
        match wake {
            Wake::Commands => {
                let batch = self.inbox.take();
                self.shell.commands(batch);
            }
            Wake::SessionEnded => self.shell.session_ended(),
            Wake::EditorClosed { tab, path, result } => {
                self.shell.editor_closed(tab, path, result);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.fit(),
            WindowEvent::Focused(focused) => self.shell.focused(focused, Instant::now()),
            // Uncovered: what was drawn while it was hidden may never
            // have been shown.
            WindowEvent::Occluded(false) => {
                self.stale = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.shell.modifiers_changed(modifiers.state());
            }
            // Not the keys that were down when the window got the
            // keyboard, which the system makes up presses for.
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.state.is_pressed() => {
                let press = Press {
                    plain: event.key_without_modifiers(),
                    text: event.text_with_all_modifiers().map(SmolStr::new),
                    key: event.logical_key,
                };
                self.shell.key_pressed(&press, Instant::now());
            }
            #[allow(clippy::cast_possible_truncation)]
            WindowEvent::CursorMoved { position, .. } => {
                self.shell
                    .pointer_moved(Point::new(position.x as f32, position.y as f32));
            }
            WindowEvent::CursorLeft { .. } => self.shell.pointer_left(),
            WindowEvent::MouseInput { state, button, .. } => {
                self.shell
                    .button(button, state.is_pressed(), Instant::now());
            }
            WindowEvent::MouseWheel { delta, .. } => self.shell.wheel(delta),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        self.shell.advance(now);
        self.act(event_loop);
        if let Some(window) = &self.window {
            let icon = self.shell.cursor_icon();
            if icon != self.icon {
                window.set_cursor(icon);
                self.icon = icon;
            }
            if self.shell.dirty() && !self.asked {
                window.request_redraw();
                self.asked = true;
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.shell.deadline(now)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    fn status(n: usize) -> UiCommand {
        UiCommand::SetStatus(n.to_string())
    }

    #[tokio::test]
    async fn commands_wake_the_loop_once_until_they_are_taken() {
        let (tx, rx) = mpsc::channel(8);
        let inbox = Arc::new(Inbox::default());
        let (wakes, ended) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        let (count, end) = (Arc::clone(&wakes), Arc::clone(&ended));
        let pump = tokio::spawn(pump(rx, Arc::clone(&inbox), move |wake| {
            match wake {
                Wake::Commands => {
                    count.fetch_add(1, Ordering::SeqCst);
                }
                _ => end.store(true, Ordering::SeqCst),
            }
            true
        }));

        for n in 0..5 {
            tx.send(status(n)).await.unwrap();
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "one wake for all five");
        assert_eq!(inbox.take().len(), 5);
        assert!(inbox.take().is_empty());

        tx.send(status(5)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
        assert_eq!(wakes.load(Ordering::SeqCst), 2, "and one for the next");
        assert_eq!(inbox.take().len(), 1);

        drop(tx);
        pump.await.unwrap();
        assert!(ended.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_full_inbox_holds_the_session_up() {
        let (tx, rx) = mpsc::channel(16);
        let inbox = Arc::new(Inbox::default());
        let pump = tokio::spawn(pump(rx, Arc::clone(&inbox), |_| true));
        let total = INBOX_ROOM + 3 * MAX_BATCH;
        let sender = tokio::spawn(async move {
            for n in 0..total {
                tx.send(status(n)).await.unwrap();
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!sender.is_finished(), "the session waits for the window");
        let mut taken = 0;
        while taken < total {
            let batch = inbox.take();
            assert!(batch.len() < INBOX_ROOM + MAX_BATCH, "{}", batch.len());
            taken += batch.len();
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(taken, total, "nothing is lost");
        sender.await.unwrap();
        pump.await.unwrap();
    }
}
