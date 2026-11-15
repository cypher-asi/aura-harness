//! Frame and update timings, logged when `AURA_DESKTOP_PERF` is set.
//!
//! Every 240 ticks this reports the mean and worst time spent applying
//! messages and rendering the terminal UI into the grid (`update`),
//! building the view (`view`) and between ticks. The first view also reports how long after launch it was
//! built, which is the cold-start number to watch.

use std::cell::Cell;
use std::time::{Duration, Instant};

const REPORT_EVERY: u32 = 240;

#[derive(Default)]
struct Window {
    count: Cell<u32>,
    total: Cell<Duration>,
    worst: Cell<Duration>,
}

impl Window {
    fn add(&self, d: Duration) {
        self.count.set(self.count.get() + 1);
        self.total.set(self.total.get() + d);
        if d > self.worst.get() {
            self.worst.set(d);
        }
    }

    fn take(&self) -> (u32, Duration, Duration) {
        let out = (self.count.get(), self.total.get(), self.worst.get());
        self.count.set(0);
        self.total.set(Duration::ZERO);
        self.worst.set(Duration::ZERO);
        out
    }
}

pub struct Perf {
    enabled: bool,
    launched: Instant,
    first_view: Cell<bool>,
    updates: Window,
    views: Window,
    frames: Window,
    last_frame: Option<Instant>,
}

impl Perf {
    #[must_use]
    pub fn new(launched: Instant) -> Self {
        Self {
            enabled: std::env::var_os("AURA_DESKTOP_PERF").is_some(),
            launched,
            first_view: Cell::new(true),
            updates: Window::default(),
            views: Window::default(),
            frames: Window::default(),
            last_frame: None,
        }
    }

    pub fn update(&self, d: Duration) {
        if self.enabled {
            self.updates.add(d);
        }
    }

    pub fn view(&self, d: Duration) {
        if self.first_view.replace(false) {
            tracing::info!(
                target: "aura_desktop::perf",
                ms = self.launched.elapsed().as_secs_f64() * 1000.0,
                "first view built after launch"
            );
        }
        if self.enabled {
            self.views.add(d);
        }
    }

    pub fn frame(&mut self, now: Instant) {
        if !self.enabled {
            return;
        }
        if let Some(last) = self.last_frame.replace(now) {
            self.frames.add(now.saturating_duration_since(last));
        }
        if self.frames.count.get() >= REPORT_EVERY {
            let ms = |d: Duration| d.as_secs_f64() * 1000.0;
            let mean = |(n, total, _): (u32, Duration, Duration)| {
                if n == 0 {
                    0.0
                } else {
                    ms(total) / f64::from(n)
                }
            };
            let (u, v, f) = (self.updates.take(), self.views.take(), self.frames.take());
            tracing::info!(
                target: "aura_desktop::perf",
                update_mean_ms = mean(u),
                update_worst_ms = ms(u.2),
                view_mean_ms = mean(v),
                view_worst_ms = ms(v.2),
                frame_mean_ms = mean(f),
                frame_worst_ms = ms(f.2),
                "frame timings"
            );
        }
    }
}
