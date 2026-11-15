//! Vector drawing on the GPU canvas: the aura in the header, activity
//! spinners and the process timeline.
//!
//! Each program is redrawn only when the app requests a frame, which
//! happens while something is live (see `Model::is_animating`); an idle
//! window draws nothing.

// Pixel math on small counts and timestamps relative to a recent start.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use crate::theme;
use aura_ui_core::events::{ProcessStatus, ProcessSummary};
use iced::alignment;
use iced::widget::canvas::{self, path::Arc, Frame, Geometry, LineCap, Path, Stroke};
use iced::{mouse, Color, Pixels, Point, Radians, Rectangle, Renderer, Size, Theme};
use std::f32::consts::TAU;

/// The AURA mark: a core with three orbiting arcs that spin and pulse
/// while the agent works.
pub struct Aura {
    /// Seconds since the app started
    pub t: f32,
    pub busy: bool,
}

impl<Message> canvas::Program<Message> for Aura {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let center = frame.center();
        let radius = bounds.width.min(bounds.height) / 2.0 - 1.0;
        let pulse = if self.busy {
            0.5 + 0.5 * (self.t * 3.0).sin()
        } else {
            0.0
        };

        for ring in 0..4 {
            let k = ring as f32 / 4.0;
            frame.fill(
                &Path::circle(center, radius * (0.45 + 0.55 * k)),
                Color {
                    a: 0.035 + 0.04 * pulse * (1.0 - k),
                    ..theme::BLUE
                },
            );
        }
        frame.fill(
            &Path::circle(center, radius * (0.24 + 0.04 * pulse)),
            Color {
                a: 0.85,
                ..theme::CYAN
            },
        );

        for (i, color) in [theme::CYAN, theme::BLUE, theme::PURPLE]
            .into_iter()
            .enumerate()
        {
            let i = i as f32;
            let speed = if self.busy { 1.4 + 0.8 * i } else { 0.0 };
            let start = self.t * speed * if i == 1.0 { -1.0 } else { 1.0 } + i * TAU / 3.0;
            let sweep = if self.busy { 1.4 + 0.6 * pulse } else { 1.6 };
            let arc = Path::new(|b| {
                b.arc(Arc {
                    center,
                    radius: radius * (0.52 + 0.16 * i),
                    start_angle: Radians(start),
                    end_angle: Radians(start + sweep),
                });
            });
            frame.stroke(
                &arc,
                Stroke::default()
                    .with_color(color)
                    .with_width(1.6)
                    .with_line_cap(LineCap::Round),
            );
        }
        vec![frame.into_geometry()]
    }
}

/// A rotating arc for work in progress.
pub struct Spinner {
    pub t: f32,
    pub color: Color,
}

impl<Message> canvas::Program<Message> for Spinner {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let center = frame.center();
        let radius = bounds.width.min(bounds.height) / 2.0 - 1.5;
        frame.stroke(
            &Path::circle(center, radius),
            Stroke::default()
                .with_color(Color {
                    a: 0.18,
                    ..self.color
                })
                .with_width(1.5),
        );
        let start = self.t * 6.0;
        let arc = Path::new(|b| {
            b.arc(Arc {
                center,
                radius,
                start_angle: Radians(start),
                end_angle: Radians(start + 1.8),
            });
        });
        frame.stroke(
            &arc,
            Stroke::default()
                .with_color(self.color)
                .with_width(1.8)
                .with_line_cap(LineCap::Round),
        );
        vec![frame.into_geometry()]
    }
}

/// Height of one process lane in the timeline.
pub const LANE_HEIGHT: f32 = 14.0;
/// Room for the time axis under the lanes.
pub const AXIS_HEIGHT: f32 = 16.0;

/// When each process ran, as bars on a shared time axis ending now.
pub struct Timeline<'a> {
    pub processes: &'a [ProcessSummary],
    pub now_ms: u64,
}

impl Timeline<'_> {
    #[must_use]
    pub fn height(lanes: usize) -> f32 {
        lanes.max(1) as f32 * LANE_HEIGHT + AXIS_HEIGHT
    }
}

impl<Message> canvas::Program<Message> for Timeline<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let width = bounds.width;
        let start_ms = self
            .processes
            .iter()
            .map(|p| p.started_ms)
            .min()
            .unwrap_or(self.now_ms)
            .min(self.now_ms.saturating_sub(60_000));
        let span_ms = (self.now_ms - start_ms).max(1) as f32;
        let x = |ms: u64| (ms.saturating_sub(start_ms) as f32 / span_ms) * width;

        for (lane, process) in self.processes.iter().enumerate() {
            let y = lane as f32 * LANE_HEIGHT;
            frame.fill_rectangle(
                Point::new(0.0, y + LANE_HEIGHT / 2.0),
                Size::new(width, 1.0),
                theme::LINE,
            );
            let x0 = x(process.started_ms);
            let x1 = x(process.ended_ms.unwrap_or(self.now_ms)).max(x0 + 3.0);
            let color = status_color(process.status);
            let bar = Path::rounded_rectangle(
                Point::new(x0, y + 3.0),
                Size::new(x1 - x0, LANE_HEIGHT - 6.0),
                3.0.into(),
            );
            frame.fill(
                &bar,
                Color {
                    a: if process.status.is_live() { 0.9 } else { 0.45 },
                    ..color
                },
            );
        }

        let axis_y = self.processes.len().max(1) as f32 * LANE_HEIGHT;
        frame.fill_rectangle(Point::new(0.0, axis_y), Size::new(width, 1.0), theme::LINE);
        let ticks = 4;
        for i in 0..=ticks {
            let fx = i as f32 / ticks as f32;
            let ago_s = ((1.0 - fx) * span_ms / 1000.0).round() as u64;
            let label = if ago_s == 0 {
                "now".to_string()
            } else if ago_s < 120 {
                format!("-{ago_s}s")
            } else {
                format!("-{}m", ago_s / 60)
            };
            frame.fill_text(canvas::Text {
                content: label,
                position: Point::new(fx * width, axis_y + 3.0),
                color: theme::FAINT,
                size: Pixels(10.0),
                font: theme::mono(),
                align_x: match i {
                    0 => alignment::Horizontal::Left.into(),
                    i if i == ticks => alignment::Horizontal::Right.into(),
                    _ => alignment::Horizontal::Center.into(),
                },
                ..canvas::Text::default()
            });
        }
        vec![frame.into_geometry()]
    }
}

#[must_use]
pub fn status_color(status: ProcessStatus) -> Color {
    match status {
        ProcessStatus::Running => theme::BLUE,
        ProcessStatus::Idle => theme::CYAN,
        ProcessStatus::Paused => theme::PURPLE,
        ProcessStatus::Exited => theme::MUTED,
        ProcessStatus::Failed | ProcessStatus::Killed => theme::RED,
    }
}
