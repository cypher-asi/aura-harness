use super::*;

// ============================================================================
// Inline
// ============================================================================

#[test]
fn inline_bold_italic_strike_and_code() {
    let spans = parse_inline(
        "a **bold** *it* _em_ ***both*** ~~gone~~ `x * y`",
        base(),
        &theme(),
    );
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "a bold it em both gone x * y");
    assert!(has(span_style(&spans, "bold"), Modifier::BOLD));
    assert!(has(span_style(&spans, "it"), Modifier::ITALIC));
    assert!(has(span_style(&spans, "em"), Modifier::ITALIC));
    let both = span_style(&spans, "both");
    assert!(has(both, Modifier::BOLD) && has(both, Modifier::ITALIC));
    assert!(has(span_style(&spans, "gone"), Modifier::CROSSED_OUT));
    let code = span_style(&spans, "x * y");
    assert_eq!(code.fg, Some(theme().colors.secondary));
    assert_eq!(code.bg, Some(theme().colors.surface));
}

#[test]
fn inline_nested_emphasis() {
    let spans = parse_inline("**bold *both* bold**", base(), &theme());
    let both = span_style(&spans, "both");
    assert!(has(both, Modifier::BOLD) && has(both, Modifier::ITALIC));
    assert!(!has(span_style(&spans, "bold "), Modifier::ITALIC));
}

#[test]
fn inline_leaves_non_emphasis_literal() {
    for text in [
        "snake_case_name and __init__ma",
        "2 * 3 * 4",
        "*args, **kwargs",
        "**unclosed",
        "~approx 5",
        "price \\*not italic\\*",
    ] {
        let spans = parse_inline(text, base(), &theme());
        let rendered: String = spans.iter().map(|s| s.content.as_ref()).collect();
        let expected = text.replace("\\*", "*");
        assert_eq!(rendered, expected, "{text}");
        assert!(
            spans.iter().all(|s| s.style == base()),
            "{text} got styled: {spans:?}"
        );
    }
}

#[test]
fn inline_emphasis_markers_inside_code_are_literal() {
    let spans = parse_inline("**use `a**b` here**", base(), &theme());
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "use a**b here");
    assert!(has(span_style(&spans, "use "), Modifier::BOLD));
}

#[test]
fn inline_links_show_label_and_url() {
    let spans = parse_inline(
        "see [the docs](https://example.com \"Docs\") or <https://a.io>",
        base(),
        &theme(),
    );
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "see the docs (https://example.com) or https://a.io");
}

#[test]
fn links_at_rest_are_gray_and_not_underlined() {
    let theme = theme();
    let body = Style::default().fg(theme.colors.text);
    let spans = parse_inline(
        "see [the docs](https://example.com) or <https://a.io>",
        body,
        &theme,
    );
    for link in ["the docs", "https://a.io"] {
        let style = span_style(&spans, link);
        assert_eq!(style.fg, Some(theme.colors.secondary), "{link}");
        assert!(!has(style, Modifier::UNDERLINED), "{link}");
    }
    // The URL after a label stays a step dimmer than the label.
    let url = span_style(&spans, " (https://example.com)");
    assert_eq!(url.fg, Some(theme.colors.muted));
    assert_ne!(theme.colors.secondary, theme.colors.muted);
}

#[test]
fn inline_link_with_url_as_label_shows_url_once() {
    let spans = parse_inline("[https://x.dev](https://x.dev)", base(), &theme());
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "https://x.dev");
}
