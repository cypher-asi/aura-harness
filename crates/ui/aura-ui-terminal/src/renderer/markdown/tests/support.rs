use super::*;

pub(super) fn theme() -> Theme {
    Theme::cyber()
}

pub(super) fn base() -> Style {
    Style::default().fg(Color::White)
}

pub(super) fn render(text: &str, width: usize) -> Vec<Line<'static>> {
    render_markdown(text, width, base(), &theme())
}

pub(super) fn texts(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

pub(super) fn span_style(spans: &[Span<'_>], content: &str) -> Style {
    spans
        .iter()
        .find(|s| s.content == content)
        .unwrap_or_else(|| panic!("no span {content:?} in {spans:?}"))
        .style
}

pub(super) fn has(style: Style, modifier: Modifier) -> bool {
    style.add_modifier.contains(modifier)
}
