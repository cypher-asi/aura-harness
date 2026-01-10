//! Simple IRC-style renderer for the terminal UI.

use crate::{
    app::{AppState, NotificationType, PanelFocus},
    components::{CodeBlock, MessageRole},
    App, Theme,
};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

// ============================================================================
// Markdown Parsing
// ============================================================================

/// Parse markdown text and return styled spans (with owned strings).
/// Supports: **bold**, *italic*, `code`, headers (#), lists (- *), blockquotes (>)
fn parse_markdown_line(text: &str, base_style: Style, theme: &Theme) -> Vec<Span<'static>> {
    // Check for line-level formatting first
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
            base_style.fg(theme.colors.secondary).add_modifier(Modifier::ITALIC),
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
    
    // Regular line - parse inline formatting
    parse_markdown_inline(text, base_style, theme)
}

/// Parse inline markdown formatting: **bold**, *italic*, `code`
fn parse_markdown_inline(text: &str, base_style: Style, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = text;
    
    while !remaining.is_empty() {
        // Try to find the next formatting marker
        if let Some((start, marker_type, marker_len)) = find_next_marker(remaining) {
            // Add text before the marker
            if start > 0 {
                spans.push(Span::styled(remaining[..start].to_string(), base_style));
            }
            
            // Find the closing marker
            let after_open = &remaining[start + marker_len..];
            let close_marker = match marker_type {
                MarkerType::Bold => "**",
                MarkerType::Italic => "*",
                MarkerType::Code => "`",
            };
            
            if let Some(close_pos) = after_open.find(close_marker) {
                let content = &after_open[..close_pos];
                let styled_content = match marker_type {
                    MarkerType::Bold => Span::styled(
                        content.to_string(),
                        base_style.add_modifier(Modifier::BOLD),
                    ),
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
                // No closing marker - treat as regular text
                spans.push(Span::styled(remaining[..start + marker_len].to_string(), base_style));
                remaining = &remaining[start + marker_len..];
            }
        } else {
            // No more markers - add remaining text
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
enum MarkerType {
    Bold,   // **
    Italic, // *
    Code,   // `
}

/// Find the next markdown marker in text
fn find_next_marker(text: &str) -> Option<(usize, MarkerType, usize)> {
    // Find ** (bold) - must check before * (italic)
    let mut best: Option<(usize, MarkerType, usize)> =
        text.find("**").map(|pos| (pos, MarkerType::Bold, 2));
    
    // Find * (italic) - only if not part of **
    for (i, c) in text.char_indices() {
        if c == '*' {
            // Check it's not part of **
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
    
    // Find ` (code)
    if let Some(pos) = text.find('`') {
        // Skip ``` code blocks (they're handled at line level)
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
enum ContentSegment {
    /// Regular text content
    Text(String),
    /// Code block with language and content
    CodeBlock { language: String, code: String },
}

/// Parse message content into segments of text and code blocks.
fn parse_content_segments(content: &str) -> Vec<ContentSegment> {
    let mut segments = Vec::new();
    let mut current_text = String::new();
    let mut in_code_block = false;
    let mut code_language = String::new();
    let mut code_content = String::new();
    
    for line in content.lines() {
        let trimmed = line.trim();
        
        if trimmed.starts_with("```") {
            if in_code_block {
                // End of code block
                segments.push(ContentSegment::CodeBlock {
                    language: std::mem::take(&mut code_language),
                    code: std::mem::take(&mut code_content),
                });
                in_code_block = false;
            } else {
                // Start of code block
                // Flush any pending text
                if !current_text.is_empty() {
                    segments.push(ContentSegment::Text(std::mem::take(&mut current_text)));
                }
                // Extract language from the opening fence
                code_language = trimmed.trim_start_matches('`').to_string();
                in_code_block = true;
            }
        } else if in_code_block {
            // Inside a code block
            if !code_content.is_empty() {
                code_content.push('\n');
            }
            code_content.push_str(line);
        } else {
            // Regular text
            if !current_text.is_empty() {
                current_text.push('\n');
            }
            current_text.push_str(line);
        }
    }
    
    // Flush remaining content
    if in_code_block {
        // Unclosed code block - treat as text
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

/// Render the full application UI in IRC style.
pub fn render(frame: &mut Frame, app: &App, theme: &Theme) {
    let area = frame.area();

    // Layout: header + content panels + input line
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header (with padding)
            Constraint::Min(3),    // Content area (panels)
            Constraint::Length(1), // Input line (with status on right)
        ])
        .split(area);

    // Render header
    render_header(frame, main_chunks[0], app, theme);

    // Render panels based on visibility
    // Layout: [Swarm (optional)] | Chat | [Record (optional)]
    render_content_panels(frame, main_chunks[1], app, theme);

    // Calculate input offset to align with chat panel when swarm is open
    let swarm_offset = if app.swarm_panel_visible() {
        // Match the swarm panel width percentages from render_content_panels
        let swarm_percent: u32 = if app.record_panel_visible() { 20 } else { 25 };
        #[expect(clippy::cast_possible_truncation, reason = "UI widths are always < u16::MAX")]
        let offset = (u32::from(main_chunks[2].width) * swarm_percent / 100) as u16;
        offset
    } else {
        0
    };

    // Render input line with status on right, offset to align with chat panel
    render_input(frame, main_chunks[2], app, theme, swarm_offset);

    // Render overlays (approval modal, help, record detail)
    render_overlays(frame, app, theme);
}

/// Render the content panels (Swarm, Chat, Record) based on visibility.
fn render_content_panels(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let swarm_visible = app.swarm_panel_visible();
    let record_visible = app.record_panel_visible();

    match (swarm_visible, record_visible) {
        (true, true) => {
            // All three panels: Swarm (20%) | Chat (50%) | Record (30%)
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(20),
                    Constraint::Percentage(50),
                    Constraint::Percentage(30),
                ])
                .split(area);
            render_swarm_panel(frame, chunks[0], app, theme);
            render_chat_panel(frame, chunks[1], app, theme);
            render_record_panel(frame, chunks[2], app, theme);
        }
        (true, false) => {
            // Swarm + Chat: Swarm (25%) | Chat (75%)
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(25),
                    Constraint::Percentage(75),
                ])
                .split(area);
            render_swarm_panel(frame, chunks[0], app, theme);
            render_chat_panel(frame, chunks[1], app, theme);
        }
        (false, true) => {
            // Chat + Record: Chat (65%) | Record (35%)
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(65),
                    Constraint::Percentage(35),
                ])
                .split(area);
            render_chat_panel(frame, chunks[0], app, theme);
            render_record_panel(frame, chunks[1], app, theme);
        }
        (false, false) => {
            // Only Chat panel (full width)
            render_chat_panel(frame, area, app, theme);
        }
    }
}

/// Render the header bar.
fn render_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    // Split header into left (title) and right (API status)
    let header_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(30)])
        .split(area);

    // Left side: AURA OS title
    let title = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("AURA", Style::default().fg(theme.colors.foreground)),
            Span::styled(" OS", Style::default().fg(theme.colors.muted)),
        ]),
        Line::from(""),
    ];
    frame.render_widget(Paragraph::new(title), header_layout[0]);

    // Right side: API status indicator (right-aligned)
    // Extra spacing around emoji for terminal rendering
    if let Some(url) = app.api_url() {
        let (icon, color) = if app.api_active() {
            (" ● ", theme.colors.primary) // Cyan dot when active
        } else {
            (" ○ ", theme.colors.error) // Red circle when inactive
        };

        let api_spans = vec![
            Span::styled(icon, Style::default().fg(color)),
            Span::styled(url, Style::default().fg(theme.colors.muted)),
        ];

        let api_status = vec![
            Line::from(""),
            Line::from(api_spans),
            Line::from(""),
        ];
        frame.render_widget(
            Paragraph::new(api_status).alignment(Alignment::Right),
            header_layout[1],
        );
    }
}

/// Render the thinking section at the bottom of the chat panel.
/// Shows the last 3 lines of thinking content in real-time.
fn render_thinking_section(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let thinking_content = app.thinking_content();
    let is_thinking = app.is_thinking();

    // Divider line at the top - left aligned like " Thinker ────────"
    let divider_width = area.width as usize;
    
    // Build label with optional spinner (extra spacing around emoji)
    let label = if is_thinking {
        format!(" Thinker  {}  ", app.spinner_char())
    } else {
        " Thinker ".to_string()
    };
    let label_len = label.chars().count();
    let right_dashes = divider_width.saturating_sub(label_len);
    
    let divider_spans = vec![
        Span::styled(
            label,
            Style::default().fg(theme.colors.muted),
        ),
        Span::styled("─".repeat(right_dashes), Style::default().fg(theme.colors.muted)),
    ];

    // Get last 3 lines of thinking content
    let content_lines: Vec<String> = if thinking_content.is_empty() {
        if is_thinking {
            vec!["...".to_string()]
        } else {
            vec![]
        }
    } else {
        // Word-wrap thinking content to fit width
        let max_width = area.width.saturating_sub(2) as usize; // 1 char padding each side
        let mut wrapped_lines: Vec<String> = Vec::new();
        
        for line in thinking_content.lines() {
            if line.is_empty() {
                wrapped_lines.push(String::new());
            } else {
                wrapped_lines.extend(wrap_words(line, max_width));
            }
        }
        
        // Get last 3 lines
        let start = wrapped_lines.len().saturating_sub(3);
        wrapped_lines.into_iter().skip(start).collect()
    };

    // Build the thinking panel content
    let mut lines = vec![Line::from(divider_spans)];
    
    // Add up to 3 lines of content
    for line_text in content_lines.iter().take(3) {
        lines.push(Line::from(vec![
            Span::styled(" ", Style::default()), // Left padding
            Span::styled(
                line_text.clone(),
                Style::default().fg(theme.colors.muted).add_modifier(Modifier::DIM),
            ),
        ]));
    }
    
    // Pad to 3 lines if needed
    while lines.len() < 4 {
        lines.push(Line::from(""));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Render the Chat panel.
#[expect(clippy::too_many_lines, reason = "UI rendering function with many visual components")]
fn render_chat_panel(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Chat;
    let border_color = if is_focused {
        theme.colors.primary
    } else {
        theme.colors.muted
    };

    let block = Block::default()
        .title(" Chat ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Check if we need thinking section (3 lines + 1 divider = 4 lines)
    let thinking_section_height = 4u16;
    let show_thinking_section = app.is_thinking() || !app.thinking_content().is_empty();

    // Split inner area: messages area and thinking section
    let (messages_area, thinking_area) = if show_thinking_section && inner.height > thinking_section_height + 3 {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(thinking_section_height),
            ])
            .split(inner);
        (split[0], Some(split[1]))
    } else {
        (inner, None)
    };

    // Render thinking section if visible
    if let Some(think_area) = thinking_area {
        render_thinking_section(frame, think_area, app, theme);
    }

    // Add padding: 2 chars left/right, 1 line top
    let padded = Rect {
        x: messages_area.x.saturating_add(2),
        y: messages_area.y.saturating_add(1),
        width: messages_area.width.saturating_sub(4),
        height: messages_area.height.saturating_sub(1),
    };

    let messages = app.messages();

    if messages.is_empty() {
        // Show simple welcome
        let welcome = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Type a message to start chatting, or /help for commands.",
                Style::default().fg(theme.colors.muted),
            )),
        ];
        let paragraph = Paragraph::new(welcome);
        frame.render_widget(paragraph, padded);
        return;
    }

    // Build IRC-style message lines with proper word wrapping
    let mut lines: Vec<Line> = Vec::new();
    let content_width = padded.width as usize;

    for message in messages.iter().skip(app.scroll_offset()) {
        // Format: [HH:MM:SS] <NICK> message
        // User: <YOU> in white, message in neon cyan
        // AURA: <AURA> in white, message in gray
        let (nick, nick_color, msg_color) = match message.role() {
            MessageRole::User => ("YOU", theme.colors.foreground, theme.colors.primary),
            MessageRole::Assistant => ("AURA", theme.colors.foreground, theme.colors.muted),
            MessageRole::System => {
                // Color system messages based on content type
                let content = message.content();
                if content.starts_with("⛔") || content.contains("Error") {
                    ("*", theme.colors.error, theme.colors.error)
                } else if content.starts_with("⚠") || content.contains("Warning") {
                    ("*", theme.colors.warning, theme.colors.warning)
                } else if content.starts_with("✓") {
                    ("*", theme.colors.success, theme.colors.success)
                } else {
                    ("*", theme.colors.muted, theme.colors.muted)
                }
            }
        };

        // Use the message's stored timestamp
        let timestamp = message.timestamp_local();

        // Calculate prefix width: "[HH:MM:SS] <NICK> " 
        // "[HH:MM:SS] " = 11 chars, "<NICK>" = nick.len() + 2 chars, " " = 1 char
        let prefix_width = 11 + nick.len() + 2 + 1; // e.g., "[12:34:56] <YOU> " = 17, "<AURA> " = 18

        // Available width for message content on first line
        let first_line_width = content_width.saturating_sub(prefix_width);
        // Continuation lines are indented to align with message text
        let continuation_width = content_width.saturating_sub(prefix_width);

        let mut is_first_output_line = true;
        
        // Parse content into segments (regular text or code blocks)
        let segments = parse_content_segments(message.content());
        
        for segment in segments {
            match segment {
                ContentSegment::Text(text) => {
                    for content_line in text.lines() {
                        if content_line.is_empty() {
                            // Empty line - just add the prefix or indent
                            if is_first_output_line {
                                lines.push(Line::from(vec![
                                    Span::styled(
                                        format!("[{timestamp}] "),
                                        Style::default().fg(theme.colors.muted),
                                    ),
                                    Span::styled(format!("<{nick}>"), Style::default().fg(nick_color)),
                                ]));
                                is_first_output_line = false;
                            } else {
                                lines.push(Line::from(""));
                            }
                            continue;
                        }

                        // Word-wrap the content line
                        let wrap_width = if is_first_output_line { first_line_width } else { continuation_width };
                        let wrapped = wrap_words(content_line, wrap_width);
                        
                        for wrapped_line in wrapped {
                            // Parse markdown for assistant messages
                            let base_style = Style::default().fg(msg_color);
                            let content_spans = if message.role() == MessageRole::Assistant {
                                parse_markdown_line(&wrapped_line, base_style, theme)
                            } else {
                                vec![Span::styled(wrapped_line, base_style)]
                            };
                            
                            if is_first_output_line {
                                // First line: include timestamp and nick
                                let mut line_spans = vec![
                                    Span::styled(
                                        format!("[{timestamp}] "),
                                        Style::default().fg(theme.colors.muted),
                                    ),
                                    Span::styled(format!("<{nick}>"), Style::default().fg(nick_color)),
                                    Span::raw(" "),
                                ];
                                line_spans.extend(content_spans);
                                lines.push(Line::from(line_spans));
                                is_first_output_line = false;
                            } else {
                                // Continuation: indent to align with message text
                                let indent = " ".repeat(prefix_width);
                                let mut line_spans = vec![Span::raw(indent)];
                                line_spans.extend(content_spans);
                                lines.push(Line::from(line_spans));
                            }
                        }
                    }
                }
                ContentSegment::CodeBlock { language, code } => {
                    // Add timestamp/nick if this is the first content
                    if is_first_output_line {
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("[{timestamp}] "),
                                Style::default().fg(theme.colors.muted),
                            ),
                            Span::styled(format!("<{nick}>"), Style::default().fg(nick_color)),
                        ]));
                        is_first_output_line = false;
                    }
                    
                    // Add padding before code block
                    lines.push(Line::from(""));
                    
                    // Render the code block with syntax highlighting
                    let code_block = CodeBlock::new(&language, &code);
                    let code_lines = code_block.render(theme, continuation_width);
                    
                    // Add code block lines with proper indentation
                    let indent = " ".repeat(prefix_width);
                    for code_line in code_lines {
                        let mut line_spans = vec![Span::raw(indent.clone())];
                        line_spans.extend(code_line.spans.into_iter().map(|s| {
                            Span::styled(s.content.to_string(), s.style)
                        }));
                        lines.push(Line::from(line_spans));
                    }
                    
                    // Add padding after code block
                    lines.push(Line::from(""));
                }
            }
        }
    }

    // Note: Thinking content is now rendered in a dedicated section at the bottom
    // of the chat panel via render_thinking_section()

    // Apply scroll offset: scroll_offset=0 means show newest at bottom
    // scroll_offset>0 means we want to see older content (scroll up)
    let visible_height = padded.height as usize;
    let scroll_offset = app.scroll_offset();
    
    // Calculate the starting line:
    // - Without scroll: start from (total - visible_height) to show newest at bottom
    // - With scroll: subtract scroll_offset to show older content
    let total_lines = lines.len();
    let bottom_start = total_lines.saturating_sub(visible_height);
    let start = bottom_start.saturating_sub(scroll_offset);
    
    let visible_lines: Vec<Line> = lines.into_iter().skip(start).take(visible_height).collect();

    let paragraph = Paragraph::new(visible_lines);
    frame.render_widget(paragraph, padded);
}

/// Calculate the display width of a string (accounting for Unicode characters).
fn display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    UnicodeWidthStr::width(s)
}

/// Wrap text at word boundaries to fit within `max_width` (display width).
/// Returns a vector of lines, each fitting within the width.
fn wrap_words(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;

    for word in text.split_whitespace() {
        let word_width = display_width(word);

        if current_width == 0 {
            // First word on line
            if word_width > max_width {
                // Word is longer than max_width, need to break it by character
                let mut chunk = String::new();
                let mut chunk_width = 0;
                for c in word.chars() {
                    use unicode_width::UnicodeWidthChar;
                    let char_width = c.width().unwrap_or(1);
                    if chunk_width + char_width > max_width && !chunk.is_empty() {
                        lines.push(std::mem::take(&mut chunk));
                        chunk_width = 0;
                    }
                    chunk.push(c);
                    chunk_width += char_width;
                }
                if !chunk.is_empty() {
                    current_line = chunk;
                    current_width = display_width(&current_line);
                }
            } else {
                current_line = word.to_string();
                current_width = word_width;
            }
        } else if current_width + 1 + word_width <= max_width {
            // Word fits on current line with space
            current_line.push(' ');
            current_line.push_str(word);
            current_width += 1 + word_width;
        } else {
            // Word doesn't fit, start new line
            lines.push(std::mem::take(&mut current_line));
            current_width = 0;

            if word_width > max_width {
                // Word is longer than max_width, need to break it by character
                let mut chunk = String::new();
                let mut chunk_width = 0;
                for c in word.chars() {
                    use unicode_width::UnicodeWidthChar;
                    let char_width = c.width().unwrap_or(1);
                    if chunk_width + char_width > max_width && !chunk.is_empty() {
                        lines.push(std::mem::take(&mut chunk));
                        chunk_width = 0;
                    }
                    chunk.push(c);
                    chunk_width += char_width;
                }
                if !chunk.is_empty() {
                    current_line = chunk;
                    current_width = display_width(&current_line);
                }
            } else {
                current_line = word.to_string();
                current_width = word_width;
            }
        }
    }

    // Don't forget the last line
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    // If no lines were created (empty or whitespace-only input), return single empty line
    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Render the Swarm panel (agent list).
fn render_swarm_panel(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Swarm;
    let border_color = if is_focused {
        theme.colors.primary
    } else {
        theme.colors.muted
    };

    let block = Block::default()
        .title(" Swarm ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let agents = app.agents();

    if agents.is_empty() {
        let empty_msg = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No agents.",
                Style::default().fg(theme.colors.muted),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Use /swarm to",
                Style::default().fg(theme.colors.muted),
            )),
            Line::from(Span::styled(
                "  manage agents.",
                Style::default().fg(theme.colors.muted),
            )),
        ];
        let paragraph = Paragraph::new(empty_msg);
        frame.render_widget(paragraph, inner);
        return;
    }

    // Build agent list - one line per agent
    let selected = app.selected_agent();
    let active_id = app.active_agent_id();
    let mut lines: Vec<Line> = Vec::new();

    for (i, agent) in agents.iter().enumerate() {
        let is_selected = i == selected;
        let is_active = agent.id == active_id;

        // Show active agent with a bullet, selected with >
        // Extra spacing around emoji for terminal rendering
        let prefix = if is_selected && is_focused {
            " > "
        } else if is_active {
            " ● "
        } else {
            "   "
        };

        let line_style = if is_selected && is_focused {
            Style::default()
                .fg(theme.colors.primary)
                .add_modifier(Modifier::BOLD)
        } else if is_active {
            Style::default().fg(theme.colors.secondary)
        } else {
            Style::default().fg(theme.colors.muted)
        };

        // Truncate name to fit panel width (using display width)
        let max_name_width = inner.width.saturating_sub(4) as usize;
        let display_name = if display_width(&agent.name) > max_name_width {
            // Truncate by display width, not bytes
            let mut truncated = String::new();
            let mut width = 0;
            for c in agent.name.chars() {
                use unicode_width::UnicodeWidthChar;
                let char_width = c.width().unwrap_or(1);
                if width + char_width >= max_name_width {
                    break;
                }
                truncated.push(c);
                width += char_width;
            }
            format!("{truncated}…")
        } else {
            agent.name.clone()
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{prefix} "), line_style),
            Span::styled(display_name, line_style),
        ]));
    }

    // Handle scrolling
    let visible_height = inner.height as usize;
    let scroll = if selected >= visible_height {
        selected.saturating_sub(visible_height / 2)
    } else {
        0
    };

    let visible_lines: Vec<Line> = lines.into_iter().skip(scroll).collect();

    let paragraph = Paragraph::new(visible_lines);
    frame.render_widget(paragraph, inner);
}

/// Render the Record panel.
fn render_record_panel(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Records;
    let border_color = if is_focused {
        theme.colors.primary
    } else {
        theme.colors.muted
    };

    let block = Block::default()
        .title(" Record ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let records = app.records();

    if records.is_empty() {
        let empty_msg = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No records yet.",
                Style::default().fg(theme.colors.muted),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Records will appear",
                Style::default().fg(theme.colors.muted),
            )),
            Line::from(Span::styled(
                "  as they are created.",
                Style::default().fg(theme.colors.muted),
            )),
        ];
        let paragraph = Paragraph::new(empty_msg);
        frame.render_widget(paragraph, inner);
        return;
    }

    // Build record list - one line per record
    // Format: # | time | ...hash | status | type | info
    let selected = app.selected_record();
    let mut lines: Vec<Line> = Vec::new();

    for (i, record) in records.iter().enumerate() {
        let is_selected = i == selected;
        let prefix = if is_selected && is_focused { ">" } else { " " };

        let line_style = if is_selected && is_focused {
            Style::default()
                .fg(theme.colors.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.colors.muted)
        };

        let hash_style = if is_selected && is_focused {
            Style::default().fg(theme.colors.secondary)
        } else {
            Style::default().fg(theme.colors.muted)
        };

        // Status indicator with color coding (extra spacing around emoji for terminal rendering)
        use crate::events::RecordStatus;
        let (status_text, status_color) = match record.status {
            RecordStatus::Ok => (" ✓ ", theme.colors.success),
            RecordStatus::Error => (" ✗ ", theme.colors.error),
            RecordStatus::Pending => (" ◌ ", theme.colors.pending),
            RecordStatus::None => (" · ", theme.colors.muted),
        };
        let status_style = if is_selected && is_focused {
            Style::default().fg(status_color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(status_color)
        };

        // Calculate available width for info: total - prefix - seq - time - status - type
        // prefix=1, seq=4, space+time+space=12, status=3 (with spacing), type=8, space=1 = 29 fixed chars
        let fixed_width = 29usize;
        let available_info_width = (inner.width as usize).saturating_sub(fixed_width);
        
        // Truncate info to fit available width
        let info_display = if record.info.is_empty() {
            String::new()
        } else if record.info.len() > available_info_width && available_info_width > 3 {
            format!("{}…", &record.info[..available_info_width.saturating_sub(1)])
        } else if record.info.len() > available_info_width {
            String::new()
        } else {
            record.info.clone()
        };

        lines.push(Line::from(vec![
            Span::styled(prefix, line_style),
            Span::styled(format!("#{:<3}", record.seq), line_style),
            Span::styled(format!(" {} ", record.timestamp), line_style),
            Span::styled(status_text, status_style),
            Span::styled(format!("{:<8}", record.tx_kind), line_style),
            Span::styled(format!(" {}", info_display), hash_style),
        ]));
    }

    // Handle scrolling for records
    let visible_height = inner.height as usize;

    // Calculate scroll offset to keep selected item visible
    let scroll = if selected >= visible_height {
        selected.saturating_sub(visible_height / 2)
    } else {
        0
    };

    let visible_lines: Vec<Line> = lines.into_iter().skip(scroll).collect();

    let paragraph = Paragraph::new(visible_lines);
    frame.render_widget(paragraph, inner);
}

/// Render input line with status on the right.
/// `left_offset` shifts the input area to align with the chat panel when swarm is visible.
fn render_input(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, left_offset: u16) {
    let input = app.input();
    let cursor_pos = app.cursor_pos();

    // Neon cyan prompt
    let prompt_color = theme.colors.primary;

    // Build status indicator for right side
    let status = app.status();
    let is_ready = status == "Ready";
    let is_thinking = status.contains("Thinking");

    // Determine status style: Ready = cyan, Thinking = gray with spinner, other = warning
    let status_style = if is_ready {
        Style::default().fg(theme.colors.primary)
    } else if is_thinking {
        Style::default().fg(theme.colors.muted)
    } else {
        Style::default().fg(theme.colors.warning)
    };

    // Use spinner for thinking, solid dot for ready, half-moon for other
    // Extra spacing around emoji for terminal rendering
    let status_text = if is_ready {
        format!(" ●  {status}")
    } else if is_thinking {
        format!(" {}  {status}", app.spinner_char())
    } else {
        format!(" ◐  {status}")
    };
    // Use display width for proper Unicode handling
    #[expect(clippy::cast_possible_truncation, reason = "status text is always short")]
    let status_len = display_width(&status_text) as u16;

    // Calculate available width for input (leave space for status on right, and apply offset)
    let input_width = area.width.saturating_sub(status_len + 2 + left_offset);

    // Render status on the right
    let status_area = Rect {
        x: area.x + area.width.saturating_sub(status_len),
        y: area.y,
        width: status_len,
        height: 1,
    };
    let status_line = Line::from(Span::styled(&status_text, status_style));
    frame.render_widget(Paragraph::new(status_line), status_area);

    // Build input line (no fake cursor - we use the real terminal cursor)
    // Only apply chat_padding when swarm panel is visible (left_offset > 0) to align with chat content
    let chat_padding: u16 = if left_offset > 0 { 3 } else { 0 };
    let input_area = Rect {
        x: area.x + left_offset + chat_padding,
        y: area.y,
        width: input_width.saturating_sub(chat_padding),
        height: 1,
    };

    let content = Line::from(vec![
        Span::styled("> ", Style::default().fg(prompt_color)),
        Span::styled(input, Style::default().fg(theme.colors.muted)),
    ]);

    frame.render_widget(Paragraph::new(content), input_area);

    // Only show cursor when not thinking (user can't type during processing)
    // Position the cursor in the input area, not at the status
    if !is_thinking {
        // Apply left_offset + chat_padding, prompt "> " is 2 chars, then cursor_pos chars into input
        #[expect(clippy::cast_possible_truncation, reason = "cursor position fits in terminal width")]
        let cursor_x = area.x + left_offset + chat_padding + 2 + cursor_pos as u16;
        let cursor_y = area.y;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

/// Render overlay elements (modals, help).
fn render_overlays(frame: &mut Frame, app: &App, theme: &Theme) {
    // Record detail overlay
    if app.showing_record_detail() {
        render_record_detail(frame, app, theme);
    }

    // Approval modal
    if let Some(approval) = app.pending_approval() {
        render_approval_modal(frame, approval, theme);
    }

    // Help overlay
    if app.state() == AppState::ShowingHelp {
        render_help_overlay(frame, theme);
    }

    // Notification
    if let Some((msg, notification_type)) = app.notification() {
        render_notification(frame, msg, *notification_type, theme);
    }
}

/// Render the approval modal.
fn render_approval_modal(frame: &mut Frame, approval: &crate::app::PendingApproval, theme: &Theme) {
    let area = frame.area();
    let modal_width = 50.min(area.width.saturating_sub(4));
    let modal_height = 6;

    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" Approval Required ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.warning));

    let content = vec![
        Line::from(vec![
            Span::styled(
                format!("{} wants to: ", approval.tool),
                Style::default().fg(theme.colors.foreground),
            ),
        ]),
        Line::from(Span::styled(
            &approval.description,
            Style::default().fg(theme.colors.muted),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("[Y] Yes", Style::default().fg(theme.colors.success)),
            Span::raw("  "),
            Span::styled("[N] No", Style::default().fg(theme.colors.error)),
        ]),
    ];

    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, modal_area);
}

/// Render the help overlay.
fn render_help_overlay(frame: &mut Frame, theme: &Theme) {
    let area = frame.area();
    let modal_width = 50.min(area.width.saturating_sub(4));
    let modal_height = 19;

    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.primary));

    let help_text = vec![
        Line::from(Span::styled(
            "/help      Show this help",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "/new       New session (reset context)",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "/clear     Clear messages",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "/swarm     Toggle Swarm panel",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "/record    Toggle Record panel",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "/quit      Exit",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Enter      Send message",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "Tab        Switch panels",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "↑/↓        Navigate / History",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "PgUp/PgDn  Scroll chat history",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "Shift+Mouse Select text to copy",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(Span::styled(
            "Ctrl+C     Cancel/Exit",
            Style::default().fg(theme.colors.foreground),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to close",
            Style::default().fg(theme.colors.muted),
        )),
    ];

    let paragraph = Paragraph::new(help_text)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, modal_area);
}

/// Render the record detail overlay.
fn render_record_detail(frame: &mut Frame, app: &App, theme: &Theme) {
    let Some(record) = app.selected_record_data() else {
        return;
    };

    let area = frame.area();
    let modal_width = 70.min(area.width.saturating_sub(4));
    // Calculate needed height based on content
    let has_error = !record.error_details.is_empty();
    let has_info = !record.info.is_empty();
    let base_height = 24u16; // Increased to fit Transaction fields
    let error_lines = if has_error { 3 + (record.error_details.len() / 60) as u16 } else { 0 };
    let modal_height = (base_height + error_lines).min(area.height.saturating_sub(4));

    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);

    // Status indicator for title (extra spacing around emoji for terminal rendering)
    use crate::events::RecordStatus;
    let (status_icon, status_color) = match record.status {
        RecordStatus::Ok => (" ✓ ", theme.colors.success),
        RecordStatus::Error => (" ✗ ", theme.colors.error),
        RecordStatus::Pending => (" ◌ ", theme.colors.pending),
        RecordStatus::None => (" · ", theme.colors.muted),
    };

    let block = Block::default()
        .title(format!(" Record #{} {} ", record.seq, status_icon))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(status_color));

    // Record info section
    let mut content = vec![
        Line::from(vec![
            Span::styled("Sequence:    ", Style::default().fg(theme.colors.muted)),
            Span::styled(
                format!("{}", record.seq),
                Style::default().fg(theme.colors.foreground),
            ),
        ]),
        Line::from(vec![
            Span::styled("Context Hash:", Style::default().fg(theme.colors.muted)),
            Span::styled(
                format!(" {}", &record.full_hash),
                Style::default().fg(theme.colors.secondary),
            ),
        ]),
    ];

    // Transaction section
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "── Transaction ──",
        Style::default().fg(theme.colors.muted),
    )));
    content.push(Line::from(vec![
        Span::styled("tx_id:       ", Style::default().fg(theme.colors.muted)),
        Span::styled(&record.tx_id, Style::default().fg(theme.colors.secondary)),
    ]));
    content.push(Line::from(vec![
        Span::styled("agent_id:    ", Style::default().fg(theme.colors.muted)),
        Span::styled(&record.agent_id, Style::default().fg(theme.colors.secondary)),
    ]));
    content.push(Line::from(vec![
        Span::styled("ts_ms:       ", Style::default().fg(theme.colors.muted)),
        Span::styled(
            format!("{}", record.ts_ms),
            Style::default().fg(theme.colors.foreground),
        ),
        Span::styled(
            format!(" ({})", record.timestamp),
            Style::default().fg(theme.colors.muted),
        ),
    ]));
    content.push(Line::from(vec![
        Span::styled("kind:        ", Style::default().fg(theme.colors.muted)),
        Span::styled(&record.tx_kind, Style::default().fg(theme.colors.foreground)),
    ]));

    // Show info if present (tool name, command, etc.)
    if has_info {
        content.push(Line::from(vec![
            Span::styled("info:        ", Style::default().fg(theme.colors.muted)),
            Span::styled(&record.info, Style::default().fg(theme.colors.primary)),
        ]));
    }

    // Processing section
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "── Processing ──",
        Style::default().fg(theme.colors.muted),
    )));
    content.push(Line::from(vec![
        Span::styled("Sender:      ", Style::default().fg(theme.colors.muted)),
        Span::styled(&record.sender, Style::default().fg(theme.colors.foreground)),
    ]));
    content.push(Line::from(vec![
        Span::styled("Actions:     ", Style::default().fg(theme.colors.muted)),
        Span::styled(
            format!("{}", record.action_count),
            Style::default().fg(theme.colors.secondary),
        ),
    ]));
    content.push(Line::from(vec![
        Span::styled("Effects:     ", Style::default().fg(theme.colors.muted)),
        Span::styled(
            &record.effect_status,
            Style::default().fg(theme.colors.foreground),
        ),
    ]));

    // Show error details if present
    if has_error {
        content.push(Line::from(""));
        content.push(Line::from(Span::styled(
            "Error Details:",
            Style::default().fg(theme.colors.error),
        )));
        // Word wrap error details
        for line in record.error_details.lines() {
            if line.is_empty() {
                content.push(Line::from(""));
            } else {
                // Simple word wrap at modal width
                let max_width = (modal_width as usize).saturating_sub(4);
                let wrapped = wrap_words(line, max_width);
                for wrapped_line in wrapped {
                    content.push(Line::from(Span::styled(
                        format!("  {wrapped_line}"),
                        Style::default().fg(theme.colors.error),
                    )));
                }
            }
        }
    }

    content.push(Line::from(""));
    content.push(Line::from(Span::styled("Message:", Style::default().fg(theme.colors.muted))));

    // Parse message content with markdown support (same as chat messages)
    let base_style = Style::default().fg(theme.colors.foreground);
    for line in record.message.lines() {
        if line.is_empty() {
            content.push(Line::from(""));
        } else {
            let spans = parse_markdown_line(line, base_style, theme);
            content.push(Line::from(spans));
        }
    }

    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "Press Esc or Enter to close",
        Style::default().fg(theme.colors.muted),
    )));

    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, modal_area);
}

/// Render a notification.
fn render_notification(
    frame: &mut Frame,
    msg: &str,
    notification_type: NotificationType,
    theme: &Theme,
) {
    let area = frame.area();
    let msg_width = u16::try_from(display_width(msg)).unwrap_or(u16::MAX);
    let toast_width = msg_width.saturating_add(6).min(area.width.saturating_sub(4));

    // Position at top-right
    let toast_area = Rect {
        x: area.width.saturating_sub(toast_width + 1),
        y: 0,
        width: toast_width,
        height: 1,
    };

    let (icon, color) = match notification_type {
        NotificationType::Success => ("✓", theme.colors.success),
        NotificationType::Warning => ("!", theme.colors.warning),
        NotificationType::Error => ("✗", theme.colors.error),
    };

    let content = Line::from(vec![
        Span::styled(format!(" {icon} "), Style::default().fg(color)),
        Span::styled(msg, Style::default().fg(theme.colors.foreground)),
    ]);

    let paragraph = Paragraph::new(content);
    frame.render_widget(paragraph, toast_area);
}

/// Helper to create a centered rect.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn test_theme() -> Theme {
        Theme::cyber()
    }

    // ========================================================================
    // Word Wrapping Tests
    // ========================================================================

    #[test]
    fn test_wrap_words_simple() {
        let result = wrap_words("hello world", 20);
        assert_eq!(result, vec!["hello world"]);
    }

    #[test]
    fn test_wrap_words_wraps_at_boundary() {
        let result = wrap_words("hello world foo bar", 11);
        assert_eq!(result, vec!["hello world", "foo bar"]);
    }

    #[test]
    fn test_wrap_words_empty_input() {
        let result = wrap_words("", 20);
        assert_eq!(result, vec![""]);
    }

    #[test]
    fn test_wrap_words_whitespace_only() {
        let result = wrap_words("   ", 20);
        assert_eq!(result, vec![""]);
    }

    #[test]
    fn test_wrap_words_long_word() {
        // Word longer than max_width should be broken by character
        let result = wrap_words("supercalifragilistic", 10);
        assert_eq!(result.len(), 2);
        assert!(result[0].len() <= 10);
    }

    #[test]
    fn test_wrap_words_zero_width() {
        let result = wrap_words("hello", 0);
        assert_eq!(result, vec!["hello"]);
    }

    #[test]
    fn test_wrap_words_exact_fit() {
        let result = wrap_words("hello", 5);
        assert_eq!(result, vec!["hello"]);
    }

    // ========================================================================
    // Display Width Tests
    // ========================================================================

    #[test]
    fn test_display_width_ascii() {
        assert_eq!(display_width("hello"), 5);
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn test_display_width_unicode() {
        // Chinese characters are typically 2 display width each
        let width = display_width("你好");
        assert!(width >= 2); // At least 2 characters
    }

    #[test]
    fn test_display_width_emoji() {
        let width = display_width("🎉");
        assert!(width >= 1);
    }

    // ========================================================================
    // Markdown Parsing Tests
    // ========================================================================

    #[test]
    fn test_parse_markdown_line_plain() {
        let theme = test_theme();
        let base_style = Style::default().fg(Color::White);

        let spans = parse_markdown_line("hello world", base_style, &theme);
        assert!(!spans.is_empty());
        // First span should contain the text
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("hello world"));
    }

    #[test]
    fn test_parse_markdown_line_header() {
        let theme = test_theme();
        let base_style = Style::default().fg(Color::White);

        let spans = parse_markdown_line("# Header", base_style, &theme);
        assert!(!spans.is_empty());
        // Headers should be bold
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_parse_markdown_line_blockquote() {
        let theme = test_theme();
        let base_style = Style::default().fg(Color::White);

        let spans = parse_markdown_line("> quoted text", base_style, &theme);
        assert!(!spans.is_empty());
        // Blockquotes should be italic
        assert!(spans[0].style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn test_parse_markdown_line_list_item() {
        let theme = test_theme();
        let base_style = Style::default().fg(Color::White);

        let spans = parse_markdown_line("- list item", base_style, &theme);
        assert!(!spans.is_empty());
        // Should have bullet character
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("•") || text.contains("-"));
    }

    #[test]
    fn test_parse_markdown_inline_bold() {
        let theme = test_theme();
        let base_style = Style::default().fg(Color::White);

        let spans = parse_markdown_inline("hello **bold** world", base_style, &theme);
        assert!(spans.len() >= 3); // "hello ", "bold", " world"
        // Find the bold span
        let bold_span = spans.iter().find(|s| s.content.as_ref() == "bold");
        assert!(bold_span.is_some());
        assert!(bold_span.unwrap().style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_parse_markdown_inline_italic() {
        let theme = test_theme();
        let base_style = Style::default().fg(Color::White);

        let spans = parse_markdown_inline("hello *italic* world", base_style, &theme);
        assert!(spans.len() >= 3);
        // Find the italic span
        let italic_span = spans.iter().find(|s| s.content.as_ref() == "italic");
        assert!(italic_span.is_some());
        assert!(italic_span.unwrap().style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn test_parse_markdown_inline_code() {
        let theme = test_theme();
        let base_style = Style::default().fg(Color::White);

        let spans = parse_markdown_inline("run `cargo build`", base_style, &theme);
        assert!(spans.len() >= 2);
        // Find the code span
        let code_span = spans.iter().find(|s| s.content.as_ref() == "cargo build");
        assert!(code_span.is_some());
    }

    #[test]
    fn test_parse_markdown_inline_unclosed() {
        let theme = test_theme();
        let base_style = Style::default().fg(Color::White);

        // Unclosed markdown should not panic
        let spans = parse_markdown_inline("hello **unclosed", base_style, &theme);
        assert!(!spans.is_empty());
    }

    // ========================================================================
    // Content Segment Parsing Tests
    // ========================================================================

    #[test]
    fn test_parse_content_segments_text_only() {
        let segments = parse_content_segments("Hello\nWorld");
        assert_eq!(segments.len(), 1);
        assert!(matches!(&segments[0], ContentSegment::Text(_)));
    }

    #[test]
    fn test_parse_content_segments_code_block() {
        let content = "Before\n```rust\nfn main() {}\n```\nAfter";
        let segments = parse_content_segments(content);

        assert_eq!(segments.len(), 3);
        assert!(matches!(&segments[0], ContentSegment::Text(_)));
        assert!(matches!(&segments[1], ContentSegment::CodeBlock { .. }));
        assert!(matches!(&segments[2], ContentSegment::Text(_)));

        if let ContentSegment::CodeBlock { language, code } = &segments[1] {
            assert_eq!(language, "rust");
            assert!(code.contains("fn main()"));
        }
    }

    #[test]
    fn test_parse_content_segments_multiple_code_blocks() {
        let content = "```python\nprint('hello')\n```\ntext\n```js\nconsole.log('hi')\n```";
        let segments = parse_content_segments(content);

        assert_eq!(segments.len(), 3);
        assert!(matches!(&segments[0], ContentSegment::CodeBlock { .. }));
        assert!(matches!(&segments[1], ContentSegment::Text(_)));
        assert!(matches!(&segments[2], ContentSegment::CodeBlock { .. }));
    }

    #[test]
    fn test_parse_content_segments_unclosed_code_block() {
        let content = "Before\n```rust\nfn main() {}";
        let segments = parse_content_segments(content);

        // Unclosed code block is flushed as text along with the "Before" segment
        // The implementation joins them together or creates multiple segments
        assert!(!segments.is_empty());
        // At least verify we get text segments (the exact count depends on implementation)
        for segment in &segments {
            assert!(matches!(segment, ContentSegment::Text(_)));
        }
    }

    // ========================================================================
    // Centered Rect Tests
    // ========================================================================

    #[test]
    fn test_centered_rect_normal() {
        let area = Rect { x: 0, y: 0, width: 100, height: 50 };
        let result = centered_rect(20, 10, area);

        assert_eq!(result.width, 20);
        assert_eq!(result.height, 10);
        assert_eq!(result.x, 40); // (100 - 20) / 2
        assert_eq!(result.y, 20); // (50 - 10) / 2
    }

    #[test]
    fn test_centered_rect_larger_than_area() {
        let area = Rect { x: 0, y: 0, width: 50, height: 30 };
        let result = centered_rect(100, 50, area);

        // Should be clamped to area size
        assert_eq!(result.width, 50);
        assert_eq!(result.height, 30);
    }

    #[test]
    fn test_centered_rect_with_offset() {
        let area = Rect { x: 10, y: 5, width: 100, height: 50 };
        let result = centered_rect(20, 10, area);

        assert_eq!(result.x, 50); // 10 + (100 - 20) / 2
        assert_eq!(result.y, 25); // 5 + (50 - 10) / 2
    }

    // ========================================================================
    // Marker Finding Tests
    // ========================================================================

    #[test]
    fn test_find_next_marker_bold() {
        let result = find_next_marker("hello **bold** world");
        assert!(result.is_some());
        let (pos, marker_type, len) = result.unwrap();
        assert_eq!(pos, 6);
        assert!(matches!(marker_type, MarkerType::Bold));
        assert_eq!(len, 2);
    }

    #[test]
    fn test_find_next_marker_italic() {
        let result = find_next_marker("hello *italic* world");
        assert!(result.is_some());
        let (pos, marker_type, len) = result.unwrap();
        assert_eq!(pos, 6);
        assert!(matches!(marker_type, MarkerType::Italic));
        assert_eq!(len, 1);
    }

    #[test]
    fn test_find_next_marker_code() {
        let result = find_next_marker("run `code` here");
        assert!(result.is_some());
        let (pos, marker_type, len) = result.unwrap();
        assert_eq!(pos, 4);
        assert!(matches!(marker_type, MarkerType::Code));
        assert_eq!(len, 1);
    }

    #[test]
    fn test_find_next_marker_none() {
        let result = find_next_marker("no markers here");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_next_marker_prefers_earliest() {
        // Bold comes before italic
        let result = find_next_marker("**bold** *italic*");
        assert!(result.is_some());
        let (pos, marker_type, _) = result.unwrap();
        assert_eq!(pos, 0);
        assert!(matches!(marker_type, MarkerType::Bold));
    }

    #[test]
    fn test_find_next_marker_skips_triple_backticks() {
        // Triple backticks are code blocks, not inline code
        let result = find_next_marker("```rust");
        assert!(result.is_none());
    }
}
