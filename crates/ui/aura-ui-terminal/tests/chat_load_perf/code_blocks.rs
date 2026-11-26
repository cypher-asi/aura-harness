//! What a fenced code block costs on its own, laid out as the chat lays it
//! out.

use crate::bench::HEIGHT;
use crate::fixtures::conversation;
use crate::markdown::fenced_code;
use crate::report::{kb, ms, print_row, ratio, threads};
use aura_ui_terminal::components::{highlight_code_line, CodeBlock};
use aura_ui_terminal::events::{MessageData, MessageRole};
use aura_ui_terminal::Theme;
use std::time::Instant;

/// What a fenced code block costs on its own, laid out as the chat lays it
/// out: the highlighter against the wrap, and the parse against the
/// token-to-colour mapping.
pub(crate) fn code_breakdown(entries: usize) {
    let replies: Vec<MessageData> = conversation(entries)
        .into_iter()
        .filter(|m| m.activity.is_none() && m.role == MessageRole::Assistant)
        .collect();
    let theme = Theme::cyber();
    // The first render is cold: it loads the syntax set and the theme.
    let blocks: Vec<String> = replies
        .iter()
        .flat_map(|m| fenced_code(&m.content))
        .collect();
    let bytes = print_blocks(entries, &blocks);
    time_renders(&blocks, &theme);
    time_highlighting(&blocks, &theme, bytes);
    time_per_kb(&blocks, &theme, bytes);
    time_grammars(&blocks, &theme);
    time_bytes_against_tokens(&theme);
    time_last_screenful(&blocks, replies.len(), &theme);
}

/// The line count of every block in `blocks` rendered as `language`, 150
/// columns wide.
fn render_all<'a>(
    blocks: impl IntoIterator<Item = &'a String>,
    language: &str,
    theme: &Theme,
) -> usize {
    blocks
        .into_iter()
        .map(|code| {
            CodeBlock::new(language, code.clone())
                .render(theme, 150)
                .len()
        })
        .sum()
}

/// Prints how much code `blocks` holds and returns its size in bytes.
fn print_blocks(entries: usize, blocks: &[String]) -> usize {
    let lines: usize = blocks.iter().map(|code| code.lines().count()).sum();
    let bytes: usize = blocks.iter().map(String::len).sum();
    println!(
        "\n{entries} entries: {} fenced rust blocks, {lines} lines, {:.1} KB of code",
        blocks.len(),
        kb(bytes),
    );
    bytes
}

fn time_renders(blocks: &[String], theme: &Theme) {
    let start = Instant::now();
    let cold = CodeBlock::new("rust", blocks[0].clone())
        .render(theme, 150)
        .len();
    print_row(
        "CodeBlock::render, cold (loads the syntax set)",
        start.elapsed(),
    );
    assert!(cold > 0);
    let start = Instant::now();
    let rendered = render_all(blocks, "rust", theme);
    print_row("CodeBlock::render, warm, highlighted", start.elapsed());
    let start = Instant::now();
    let plain = render_all(blocks, "", theme);
    print_row("CodeBlock::render, warm, no language", start.elapsed());
    assert_eq!(plain, rendered);
}

/// Just the highlighting, line by line, the way a block does it.
fn time_highlighting(blocks: &[String], theme: &Theme, bytes: usize) {
    let start = Instant::now();
    let mut span_count = 0usize;
    for code in blocks {
        for line in code.lines() {
            span_count += highlight_code_line("rust", line, theme).len();
        }
    }
    print_row("highlight_code_line, warm, every line", start.elapsed());
    println!(
        "    {span_count} spans over {bytes} bytes, {:.1} bytes per span",
        ratio(bytes, span_count)
    );
}

/// The number to plan with: how long a kilobyte of fenced rust costs.
fn time_per_kb(blocks: &[String], theme: &Theme, bytes: usize) {
    let start = Instant::now();
    let n = render_all(blocks, "rust", theme);
    let elapsed = start.elapsed();
    assert!(n > 0);
    let per_kb = ms(elapsed) / kb(bytes);
    println!(
        "    {per_kb:.2} ms per KB of fenced rust, one thread; {:.2} ms across {} threads",
        per_kb / f64::from(threads()),
        threads()
    );
}

/// The same lines under other grammars: the parse is grammar-specific,
/// the token-to-colour mapping is not, so a cheaper grammar costing much
/// less says the parse is where the time goes.
fn time_grammars(blocks: &[String], theme: &Theme) {
    for language in ["rust", "yaml", "json", "txt"] {
        let start = Instant::now();
        let n = render_all(blocks, language, theme);
        print_row(
            &format!("the same lines parsed as {language}"),
            start.elapsed(),
        );
        assert!(n > 0);
    }
}

/// Bytes against tokens: lines with one token each, then the same bytes
/// in many small tokens. Costing the same says the time goes by byte of
/// code parsed, not by span produced.
fn time_bytes_against_tokens(theme: &Theme) {
    let wide: String = (0..10_000)
        .map(|i| format!("identifier_number_{i}\n"))
        .collect();
    let dense: String = (0..10_000)
        .map(|i| format!("let a{i} = f(b{i}, c{i})?;\n"))
        .collect();
    println!(
        "\n{} bytes one token a line, {} bytes many tokens a line",
        wide.len(),
        dense.len()
    );
    for (label, code) in [("one token a line", wide), ("many tokens a line", dense)] {
        let start = Instant::now();
        let n = CodeBlock::new("rust", code).render(theme, 150).len();
        print_row(&format!("10k lines, {label}"), start.elapsed());
        assert!(n > 0);
    }
}

/// What the fix would put on the first frame: the blocks of the last
/// screenful, which is all the panel can show, against all of them.
fn time_last_screenful(blocks: &[String], replies: usize, theme: &Theme) {
    let per_reply = blocks.len() / replies.max(1);
    let visible_replies = HEIGHT as usize / 4;
    let window: Vec<&String> = blocks
        .iter()
        .skip(blocks.len().saturating_sub(visible_replies * per_reply))
        .collect();
    let start = Instant::now();
    let n = render_all(window.iter().copied(), "rust", theme);
    print_row(
        &format!(
            "only the last {visible_replies} replies' code ({} blocks)",
            window.len()
        ),
        start.elapsed(),
    );
    assert!(n > 0);
}
