//! The lines of one chat message.

use super::stream::Laid;
use super::timeline::is_notice;
use crate::components::{Message, MessageRole, MoreSection};
use crate::renderer::activity::{activity_lines, Shown};
use crate::renderer::boot::{boot_copy, boot_lines};
use crate::renderer::command::shell_lines;
use crate::renderer::diff::diff_message_lines;
use crate::renderer::markdown::{parse_content_segments, ContentSegment};
use crate::renderer::segments::{
    message_style, prefixed_lines, render_code_segment, render_text_segment,
};
use crate::renderer::text::Measure;
use crate::renderer::MoreRows;
use crate::Theme;

/// The lines of one chat message, whether each is prose, the copy
/// buttons of its code blocks, and its rows that show more of a part cut
/// short. What the user sent follows a `> ` prompt and system messages a
/// `* `; the agent's replies stand on their own, like a program's output.
pub(super) fn message_lines(
    message: &Message,
    (width, measure): (usize, Measure),
    theme: &Theme,
    spinner: &str,
) -> (Laid, MoreRows) {
    let alone = |lines| (Laid::of(lines), MoreRows::new());
    let with_more = |(lines, more)| (Laid::of(lines), more);
    if let Some(activity) = message.activity_step() {
        return with_more(activity_lines(
            activity,
            Shown::of(message),
            None,
            spinner,
            width,
            theme,
        ));
    }
    if let Some(run) = message.shell_run() {
        return alone(shell_lines(run, &message.timestamp_local(), width, theme));
    }
    if let Some(block) = message.diff_block() {
        let more = message.more_lines(MoreSection::Diff);
        return with_more(diff_message_lines(block, more, width, theme));
    }
    if let Some(console) = message.boot_console() {
        let (lines, more) = boot_lines(console, message.is_expanded(), spinner, width, theme);
        let copy = boot_copy(console, width).into_iter().collect();
        let laid = Laid {
            codes: copy,
            ..Laid::of(lines)
        };
        return (laid, more);
    }
    let role = message.role();
    if let Some(why) = error_text(message) {
        return alone(error_lines(why, width, theme));
    }
    let (prompt, prompt_color, msg_color) = message_style(role, theme);
    if is_notice(message) {
        return alone(prefixed_lines(
            (prompt, prompt_color),
            message.content(),
            msg_color,
            width,
            Measure::GRID,
        ));
    }
    (
        content_lines(message.content(), role, (width, measure), theme),
        MoreRows::new(),
    )
}

/// The lines of the text a message of `role` says, whether each is prose,
/// the copy buttons of its code blocks and its links: fenced code set
/// apart from the text around it.
pub(super) fn content_lines(
    content: &str,
    role: MessageRole,
    (width, measure): (usize, Measure),
    theme: &Theme,
) -> Laid {
    let (prompt, prompt_color, msg_color) = message_style(role, theme);
    let mut laid = Laid::default();
    for (i, segment) in parse_content_segments(content).into_iter().enumerate() {
        let mut part = Laid::default();
        match segment {
            ContentSegment::Text(text) => {
                let (prose, links) = render_text_segment(
                    &mut part.lines,
                    &text,
                    (role, i == 0),
                    width,
                    theme,
                    measure,
                );
                part.prose = prose;
                part.links = links;
            }
            ContentSegment::CodeBlock { language, code } => {
                if i == 0 && !prompt.is_empty() {
                    part.lines.extend(prefixed_lines(
                        (prompt, prompt_color),
                        "",
                        msg_color,
                        width,
                        Measure::GRID,
                    ));
                }
                part.codes.extend(render_code_segment(
                    &mut part.lines,
                    &language,
                    &code,
                    width,
                    theme,
                ));
            }
        }
        laid.add(part);
    }
    laid
}

/// Before an error in the chat, in the Stop button's red.
pub(in crate::renderer) const ERROR_PROMPT: &str = "✗ ";

/// The lines of an error `why`: `✗ ` then the reason, in the Stop
/// button's red.
fn error_lines(why: &str, width: usize, theme: &Theme) -> Vec<ratatui::text::Line<'static>> {
    let red = crate::renderer::failure_color(theme);
    prefixed_lines((ERROR_PROMPT, red), why, red, width, Measure::GRID)
}

/// What an error the harness showed in the chat (a system message made
/// by `UiCommand::ShowError`) says, past its `error: ` prefix.
fn error_text(message: &Message) -> Option<&str> {
    if message.role() != MessageRole::System {
        return None;
    }
    message.content().strip_prefix(crate::events::ERROR_PREFIX)
}
