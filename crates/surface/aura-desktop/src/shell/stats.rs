//! What the window spends its time on, logged when `AURA_DESKTOP_PERF`
//! is set.
//!
//! Every five seconds in which something was drawn or handled, this
//! reports the mean and worst time spent handling what the window was
//! told (`update`), rendering the terminal UI into the grid (`render`),
//! turning it into quads (`paint`) and drawing and showing them
//! (`show`), how many frames were shown and how many came out the same
//! as the one on screen and were not, how many grid rows were painted
//! anew, and how much of each second the window's thread was busy in
//! all (frames shown on the GPU are shown from a thread of their own, so
//! their `show` is left out of it). The first frame also reports how
//! long after launch it was shown, which is the cold-start number to
//! watch.

use std::time::{Duration, Instant};

const REPORT_EVERY: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Span {
    count: u32,
    total: Duration,
    worst: Duration,
}

impl Span {
    fn add(&mut self, d: Duration) {
        self.count += 1;
        self.total += d;
        self.worst = self.worst.max(d);
    }

    fn mean_ms(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            ms(self.total) / f64::from(self.count)
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

pub struct Stats {
    enabled: bool,
    launched: Instant,
    first_frame: bool,
    /// When the spans below were last reported
    since: Instant,
    updates: Span,
    renders: Span,
    paints: Span,
    shows: Span,
    /// Frames that came out as the one on screen
    same: u32,
    /// Grid rows painted anew
    rows: usize,
    /// Time in `shows` spent on another thread
    away: Duration,
}

impl Stats {
    #[must_use]
    pub fn new(launched: Instant) -> Self {
        Self {
            enabled: std::env::var_os("AURA_DESKTOP_PERF").is_some(),
            launched,
            first_frame: true,
            since: Instant::now(),
            updates: Span::default(),
            renders: Span::default(),
            paints: Span::default(),
            shows: Span::default(),
            same: 0,
            rows: 0,
            away: Duration::ZERO,
        }
    }

    /// Something the window was told took `d` to handle.
    pub fn update(&mut self, d: Duration) {
        if self.enabled {
            self.updates.add(d);
        }
    }

    /// A frame took `render` to render the terminal UI and `paint` to
    /// turn into quads, `rows` grid rows of them anew.
    pub fn frame(&mut self, render: Duration, paint: Duration, rows: usize) {
        if self.enabled {
            self.renders.add(render);
            self.paints.add(paint);
            self.rows += rows;
        }
    }

    /// The frame was drawn and shown in `d`, or was the one on screen
    /// again and not shown (`None`).
    pub fn shown(&mut self, d: Option<Duration>) {
        if d.is_some() && std::mem::take(&mut self.first_frame) {
            tracing::info!(
                target: "aura_desktop::perf",
                ms = ms(self.launched.elapsed()),
                "first frame shown after launch"
            );
        }
        if !self.enabled {
            return;
        }
        match d {
            Some(d) => self.shows.add(d),
            None => self.same += 1,
        }
    }

    /// A frame was drawn and shown in `d` by the thread that shows
    /// frames, which kept the window's thread free meanwhile.
    pub fn shown_away(&mut self, d: Duration) {
        self.shown(Some(d));
        if self.enabled {
            self.away += d;
        }
    }

    /// Log the spans when they are due.
    pub fn report(&mut self, now: Instant) {
        let over = now.saturating_duration_since(self.since);
        if !self.enabled || over < REPORT_EVERY {
            return;
        }
        let spans = [&self.updates, &self.renders, &self.paints, &self.shows];
        let busy = spans
            .iter()
            .map(|span| span.total)
            .sum::<Duration>()
            .saturating_sub(self.away);
        if busy > Duration::ZERO {
            tracing::info!(
                target: "aura_desktop::perf",
                update_mean_ms = self.updates.mean_ms(),
                update_worst_ms = ms(self.updates.worst),
                render_mean_ms = self.renders.mean_ms(),
                render_worst_ms = ms(self.renders.worst),
                paint_mean_ms = self.paints.mean_ms(),
                paint_worst_ms = ms(self.paints.worst),
                show_mean_ms = self.shows.mean_ms(),
                show_worst_ms = ms(self.shows.worst),
                frames_shown = self.shows.count,
                frames_same = self.same,
                rows_painted = self.rows,
                busy_ms_per_s = ms(busy) / over.as_secs_f64(),
                "frame timings"
            );
        }
        self.since = now;
        self.updates = Span::default();
        self.renders = Span::default();
        self.paints = Span::default();
        self.shows = Span::default();
        self.same = 0;
        self.rows = 0;
        self.away = Duration::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_keep_the_mean_and_the_worst() {
        let mut span = Span::default();
        assert!(span.mean_ms().abs() < f64::EPSILON);
        span.add(Duration::from_millis(2));
        span.add(Duration::from_millis(6));
        assert!((span.mean_ms() - 4.0).abs() < 1e-9);
        assert_eq!(span.worst, Duration::from_millis(6));
    }
}
