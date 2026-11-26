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
use ratatui::text::Line;

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
    let alone = |lines| ((lines, Vec::new(), Vec::new()), MoreRows::new());
    let with_more = |(lines, more)| ((lines, Vec::new(), Vec::new()), more);
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
        return ((lines, Vec::new(), copy), more);
    }
    let role = message.role();
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
/// and the copy buttons of its code blocks: fenced code set apart from
/// the text around it.
pub(super) fn content_lines(
    content: &str,
    role: MessageRole,
    (width, measure): (usize, Measure),
    theme: &Theme,
) -> Laid {
    let (prompt, prompt_color, msg_color) = message_style(role, theme);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut prose = Vec::new();
    let mut codes = Vec::new();
    for (i, segment) in parse_content_segments(content).into_iter().enumerate() {
        prose.resize(lines.len(), None);
        match segment {
            ContentSegment::Text(text) => {
                prose.extend(render_text_segment(
                    &mut lines,
                    &text,
                    (role, i == 0),
                    width,
                    theme,
                    measure,
                ));
            }
            ContentSegment::CodeBlock { language, code } => {
                if i == 0 && !prompt.is_empty() {
                    lines.extend(prefixed_lines(
                        (prompt, prompt_color),
                        "",
                        msg_color,
                        width,
                        Measure::GRID,
                    ));
                }
                codes.extend(render_code_segment(
                    &mut lines, &language, &code, width, theme,
                ));
            }
        }
    }
    (lines, prose, codes)
}
