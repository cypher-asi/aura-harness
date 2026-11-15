//! Widget tree for each frame, built from [`App`] state.

mod chrome;
mod overlays;
mod side;
mod transcript;

use crate::app::{App, Message};
use crate::{ansi, draw, theme};
use iced::widget::text::{Span, Wrapping};
use iced::widget::{
    button, canvas, center, column, container, opaque, rich_text, row, stack, text,
};
use iced::{Color, Element, Fill, Font, Length};

pub fn view(app: &App) -> Element<'_, Message> {
    let main = column![transcript::view(app), chrome::input(app)]
        .width(Fill)
        .height(Fill);
    let mut body = row![main].height(Fill);
    if let Some(tab) = app.side {
        body = body.push(side::view(app, tab));
    }
    let base = container(column![chrome::header(app), body, chrome::status_bar(app)])
        .style(theme::app_background)
        .width(Fill)
        .height(Fill);

    match overlays::view(app) {
        Some(modal) => stack![base, opaque(center(modal).style(theme::scrim))].into(),
        None => base.into(),
    }
}

/// Small secondary text.
pub(crate) fn muted<'a>(content: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(content).size(theme::SMALL_SIZE).color(theme::MUTED)
}

/// Upper-case heading of a panel section.
pub(crate) fn section<'a>(title: &'a str) -> text::Text<'a> {
    text(title)
        .size(11)
        .font(theme::ui_semibold())
        .color(theme::FAINT)
}

/// One line of mono text that never wraps.
pub(crate) fn mono_line<'a>(content: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(content)
        .font(theme::mono())
        .size(theme::MONO_SIZE)
        .wrapping(Wrapping::None)
}

pub(crate) fn spinner<'a>(app: &App, size: f32, color: Color) -> Element<'a, Message> {
    canvas(draw::Spinner {
        t: app.clock(),
        color,
    })
    .width(size)
    .height(size)
    .into()
}

/// A filled dot, e.g. for status.
pub(crate) fn dot<'a>(color: Color) -> Element<'a, Message> {
    container(text(""))
        .width(8)
        .height(8)
        .style(move |_| container::Style {
            background: Some(color.into()),
            border: iced::border::rounded(4),
            ..container::Style::default()
        })
        .into()
}

pub(crate) fn small_button<'a>(
    label: &'a str,
    color: Color,
    message: Message,
) -> Element<'a, Message> {
    button(text(label).size(11))
        .padding([3, 8])
        .style(theme::accent(color))
        .on_press(message)
        .into()
}

/// Command output with its ANSI colors.
pub(crate) fn ansi_text<'a>(output: &str) -> Element<'a, Message> {
    let spans: Vec<Span<'static, (), Font>> = ansi::parse(output)
        .into_iter()
        .map(|run| {
            let color = run.color.unwrap_or(theme::TEXT);
            let color = if run.dim {
                Color { a: 0.6, ..color }
            } else {
                color
            };
            Span::new(run.text).color(color).font(if run.bold {
                theme::mono_medium()
            } else {
                theme::mono()
            })
        })
        .collect();
    rich_text(spans).size(theme::MONO_SIZE).into()
}

/// The first `max_lines` lines of `text`, and how many were left out.
pub(crate) fn clip_lines(text: &str, max_lines: usize) -> (&str, usize) {
    let mut end = text.len();
    let mut lines = 0;
    for (i, c) in text.char_indices() {
        if c == '\n' {
            lines += 1;
            if lines == max_lines {
                end = i;
                break;
            }
        }
    }
    if end == text.len() {
        return (text, 0);
    }
    let hidden = text[end..].lines().count().saturating_sub(1).max(1);
    (&text[..end], hidden)
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

pub(crate) fn fixed(width: f32) -> Length {
    Length::Fixed(width)
}

/// Leading space for a row nested `depth` levels deep.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn indent<'a>(depth: usize, step: f32) -> Element<'a, Message> {
    iced::widget::space()
        .width(fixed(depth as f32 * step))
        .into()
}

pub(crate) fn pad(top: f32, right: f32, bottom: f32, left: f32) -> iced::Padding {
    iced::Padding {
        top,
        right,
        bottom,
        left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_lines_counts_what_it_hides() {
        assert_eq!(clip_lines("a\nb\nc", 5), ("a\nb\nc", 0));
        let (shown, hidden) = clip_lines("1\n2\n3\n4\n5", 2);
        assert_eq!(shown, "1\n2");
        assert_eq!(hidden, 3);
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(340), "340ms");
        assert_eq!(duration(2_340), "2.3s");
        assert_eq!(duration(125_000), "2m 5s");
    }
}
