//! Markdown parsing for the terminal renderer.

use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::Span,
};

/// Parse markdown text and return styled spans (with owned strings).
/// Supports: **bold**, *italic*, `code`, headers (#), lists (- *), blockquotes (>)
pub(crate) fn parse_markdown_line(
    text: &str,
    base_style: Style,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let trimmed = text.trim_start();

    // Headers: # ## ###
    if trimmed.starts_with("# ") {
        return vec![Span::styled(
            text.to_string(),
            base_style.add_modifier(Modifier::BOLD),
        )];
    }
    if trimmed.starts_with("## ") || trimmed.starts_with("### ") {
        return vec![Span::styled(
            text.to_string(),
            base_style.add_modifier(Modifier::BOLD),
        )];
    }

    // Blockquotes: > text
    if trimmed.starts_with("> ") {
        return vec![Span::styled(
            text.to_string(),
            base_style
                .fg(theme.colors.secondary)
                .add_modifier(Modifier::ITALIC),
        )];
    }

    // List items: - item or * item
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        let indent = text.len() - trimmed.len();
        let bullet_span = Span::styled(
            format!("{}• ", " ".repeat(indent)),
            base_style.fg(theme.colors.primary),
        );
        let rest = &trimmed[2..];
        let mut result = vec![bullet_span];
        result.extend(parse_markdown_inline(rest, base_style, theme));
        return result;
    }

    // Numbered lists: 1. 2. etc
    if let Some(dot_pos) = trimmed.find(". ") {
        if dot_pos <= 3 && trimmed[..dot_pos].chars().all(|c| c.is_ascii_digit()) {
            let indent = text.len() - trimmed.len();
            let number = &trimmed[..=dot_pos];
            let number_span = Span::styled(
                format!("{}{} ", " ".repeat(indent), number),
                base_style.fg(theme.colors.primary),
            );
            let rest = &trimmed[dot_pos + 2..];
            let mut result = vec![number_span];
            result.extend(parse_markdown_inline(rest, base_style, theme));
            return result;
        }
    }

    parse_markdown_inline(text, base_style, theme)
}

/// Parse inline markdown formatting: **bold**, *italic*, `code`
pub(super) fn parse_markdown_inline(
    text: &str,
    base_style: Style,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if let Some((start, marker_type, marker_len)) = find_next_marker(remaining) {
            if start > 0 {
                spans.push(Span::styled(remaining[..start].to_string(), base_style));
            }

            let after_open = &remaining[start + marker_len..];
            let close_marker = match marker_type {
                MarkerType::Bold => "**",
                MarkerType::Italic => "*",
                MarkerType::Code => "`",
            };

            if let Some(close_pos) = after_open.find(close_marker) {
                let content = &after_open[..close_pos];
                let styled_content = match marker_type {
                    MarkerType::Bold => {
                        Span::styled(content.to_string(), base_style.add_modifier(Modifier::BOLD))
                    }
                    MarkerType::Italic => Span::styled(
                        content.to_string(),
                        base_style.add_modifier(Modifier::ITALIC),
                    ),
                    MarkerType::Code => Span::styled(
                        content.to_string(),
                        Style::default()
                            .fg(theme.colors.success)
                            .bg(theme.colors.background),
                    ),
                };
                spans.push(styled_content);
                remaining = &after_open[close_pos + close_marker.len()..];
            } else {
                spans.push(Span::styled(
                    remaining[..start + marker_len].to_string(),
                    base_style,
                ));
                remaining = &remaining[start + marker_len..];
            }
        } else {
            spans.push(Span::styled(remaining.to_string(), base_style));
            break;
        }
    }

    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base_style));
    }

    spans
}

#[derive(Debug, Clone, Copy)]
pub(super) enum MarkerType {
    Bold,
    Italic,
    Code,
}

/// Find the next markdown marker in text
pub(super) fn find_next_marker(text: &str) -> Option<(usize, MarkerType, usize)> {
    let mut best: Option<(usize, MarkerType, usize)> =
        text.find("**").map(|pos| (pos, MarkerType::Bold, 2));

    for (i, c) in text.char_indices() {
        if c == '*' {
            let is_double = text[i..].starts_with("**");
            let prev_is_star = i > 0 && text.as_bytes().get(i - 1) == Some(&b'*');

            if !is_double && !prev_is_star {
                let italic_pos = i;
                match best {
                    Some((best_pos, _, _)) if best_pos <= italic_pos => {}
                    _ => best = Some((italic_pos, MarkerType::Italic, 1)),
                }
                break;
            }
        }
    }

    if let Some(pos) = text.find('`') {
        if !text[pos..].starts_with("```") {
            match best {
                Some((best_pos, _, _)) if best_pos <= pos => {}
                _ => best = Some((pos, MarkerType::Code, 1)),
            }
        }
    }

    best
}

// ============================================================================
// Code Block Parsing
// ============================================================================

/// Content segment - either regular text or a code block.
#[derive(Debug)]
pub(crate) enum ContentSegment {
    Text(String),
    CodeBlock { language: String, code: String },
}

/// Parse message content into segments of text and code blocks.
pub(crate) fn parse_content_segments(content: &str) -> Vec<ContentSegment> {
    let mut segments = Vec::new();
    let mut current_text = String::new();
    let mut in_code_block = false;
    let mut code_language = String::new();
    let mut code_content = String::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            if in_code_block {
                segments.push(ContentSegment::CodeBlock {
                    language: std::mem::take(&mut code_language),
                    code: std::mem::take(&mut code_content),
                });
                in_code_block = false;
            } else {
                if !current_text.is_empty() {
                    segments.push(ContentSegment::Text(std::mem::take(&mut current_text)));
                }
                code_language = trimmed.trim_start_matches('`').to_string();
                in_code_block = true;
            }
        } else if in_code_block {
            if !code_content.is_empty() {
                code_content.push('\n');
            }
            code_content.push_str(line);
        } else {
            if !current_text.is_empty() {
                current_text.push('\n');
            }
            current_text.push_str(line);
        }
    }

    // Flush remaining content
    if in_code_block {
        if current_text.is_empty() {
            current_text = format!("```{code_language}");
        } else {
            current_text.push_str("\n```");
            current_text.push_str(&code_language);
        }
        if !code_content.is_empty() {
            current_text.push('\n');
            current_text.push_str(&code_content);
        }
    }

    if !current_text.is_empty() {
        segments.push(ContentSegment::Text(current_text));
    }

    segments
}
