use super::*;

fn text_of(spans: &[Span<'_>]) -> String {
    spans.iter().map(|s| s.content.as_ref()).collect()
}

fn highlight_all(syntax: &SyntaxReference, source: &str) -> Vec<Vec<Span<'static>>> {
    let theme = Theme::cyber();
    let mut highlighter = LineHighlighter::new(syntax);
    source
        .lines()
        .map(|line| highlighter.spans(line, &theme))
        .collect()
}

fn color_of(spans: &[Span<'_>], text: &str) -> Option<ratatui::style::Color> {
    spans
        .iter()
        .find(|s| s.content.trim() == text)
        .and_then(|s| s.style.fg)
}

fn file_syntax(name: &str) -> &'static SyntaxReference {
    syntax_for_path(Path::new(name), "")
}

#[test]
fn every_rule_is_a_valid_selector() {
    for (selector, _) in RULES {
        assert!(selector.parse::<ScopeSelectors>().is_ok(), "{selector}");
    }
}

#[test]
fn sentinels_round_trip() {
    for role in ROLES {
        assert_eq!(Role::from_sentinel(role.sentinel()), role);
    }
}

#[test]
fn html_style_block_highlights_css() {
    let html =
        "<html>\n<head>\n<style>\n  body {\n    color: red;\n  }\n</style>\n</head>\n</html>\n";
    let lines = highlight_all(file_syntax("index.html"), html);
    let css = &lines[4];
    assert_eq!(text_of(css), "    color: red;");
    assert_eq!(color_of(css, "color"), Some(BLUE), "{css:?}");
    assert_eq!(color_of(css, "red"), Some(CYAN), "{css:?}");
    assert_eq!(color_of(&lines[2], "style"), Some(PURPLE), "{:?}", lines[2]);
}

#[test]
fn rust_block_comment_spans_lines() {
    let source = "/* start\n   still a comment\n*/\nfn main() {}\n";
    let lines = highlight_all(file_syntax("lib.rs"), source);
    assert!(
        lines[1].iter().all(|s| s.style.fg == Some(GRAY)),
        "{:?}",
        lines[1]
    );
    assert_eq!(color_of(&lines[3], "fn"), Some(PURPLE));
    assert_eq!(color_of(&lines[3], "main"), Some(BLUE));
}

#[test]
fn toml_and_yaml_keys_and_strings() {
    let lines = highlight_all(file_syntax("Cargo.toml"), "[package]\nname = \"aura\"\n");
    assert_eq!(color_of(&lines[0], "package"), Some(CYAN), "{:?}", lines[0]);
    assert_eq!(color_of(&lines[1], "name"), Some(BLUE), "{:?}", lines[1]);
    assert_eq!(
        color_of(&lines[1], "\"aura\""),
        Some(CYAN),
        "{:?}",
        lines[1]
    );
    let lines = highlight_all(file_syntax("ci.yml"), "key: value\n");
    assert_eq!(color_of(&lines[0], "key"), Some(BLUE), "{:?}", lines[0]);
    assert_eq!(color_of(&lines[0], "value"), Some(CYAN), "{:?}", lines[0]);
}

#[test]
fn markdown_heading_is_bold_primary() {
    let theme = Theme::cyber();
    let lines = highlight_all(file_syntax("README.md"), "# Title\n\nbody\n");
    let heading = lines[0]
        .iter()
        .find(|s| s.content.contains("Title"))
        .unwrap();
    assert_eq!(heading.style.fg, Some(theme.colors.primary));
    assert!(heading.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn unknown_extension_is_plain_and_exact() {
    let theme = Theme::cyber();
    let syntax = syntax_for_path(Path::new("notes.zzunknown"), "just words");
    let spans = highlight_line(syntax, "  let x = \"y\"; // z", &theme);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content, "  let x = \"y\"; // z");
    assert_eq!(spans[0].style.fg, Some(theme.colors.foreground));
}

#[test]
fn spans_always_concatenate_to_the_line() {
    let samples = [
        ("a.rs", "fn main() { let s = \"x\\ty\"; } // é"),
        ("a.py", "def f(x): return f'{x!r}'  # ok"),
        ("a.ts", "const f = (a: number): string => `${a}`;"),
        ("a.html", "<a href=\"x\">link</a> &amp;"),
        ("a.css", ".x > a:hover { margin: 0 auto; }"),
        ("a.json", "{\"k\": [1, true, null]}"),
        ("a.yaml", "key: - value # c"),
        ("a.sql", "SELECT * FROM t WHERE a = 'b';"),
        ("a.c", "#include <stdio.h> int x = 0x1f;"),
        ("a.md", "**bold** _it_ `code` [l](u)"),
        ("a.sh", "echo \"$HOME\" | grep -v x"),
        ("Dockerfile", "FROM rust:1.85 AS build"),
        ("a.go", "func main() { fmt.Println(\"hi\") }"),
        ("a.java", "public static void main(String[] a) {}"),
        ("a.diff", "+added line"),
        ("a.rs", ""),
    ];
    let theme = Theme::cyber();
    for (name, line) in samples {
        let syntax = file_syntax(name);
        assert_eq!(
            text_of(&highlight_line(syntax, line, &theme)),
            line,
            "{name}"
        );
    }
}

#[test]
fn files_are_detected_by_name_extension_and_first_line() {
    assert_eq!(file_syntax("Dockerfile").name, "Dockerfile");
    assert_eq!(file_syntax("Dockerfile.render").name, "Dockerfile");
    assert_eq!(file_syntax("Makefile").name, "Makefile");
    assert_eq!(file_syntax("CMakeLists.txt").name, "CMake");
    assert_eq!(file_syntax("x.toml").name, "TOML");
    assert_eq!(file_syntax("x.tsx").name, "TypeScriptReact");
    assert_eq!(
        syntax_for_path(Path::new("script"), "#!/bin/bash").name,
        "Bourne Again Shell (bash)"
    );
}

#[test]
fn tokens_resolve_names_and_extensions() {
    for (token, name) in [
        ("rust", "Rust"),
        ("rs", "Rust"),
        ("python", "Python"),
        ("typescript", "TypeScript"),
        ("tsx", "TypeScriptReact"),
        ("yml", "YAML"),
        ("sh", "Bourne Again Shell (bash)"),
        ("zsh", "Bourne Again Shell (bash)"),
        ("json", "JSON"),
    ] {
        assert_eq!(
            syntax_for_token(token).map(|s| s.name.as_str()),
            Some(name),
            "{token}"
        );
    }
    assert!(syntax_for_token("").is_none());
    assert!(syntax_for_token("definitely-not-a-language").is_none());
}

#[test]
fn cache_far_down_matches_highlighting_from_scratch() {
    let source: String = (0..6000)
        .map(|n| match n % 7 {
            0 => "/* a comment\n".to_string(),
            1 => format!("   spanning {n} lines */\n"),
            2 => format!("fn f{n}(x: u32) -> u32 {{ x + {n} }}\n"),
            3 => "let s = \"multi\n".to_string(),
            4 => "line string\";\n".to_string(),
            _ => format!("// note {n}\n"),
        })
        .collect();
    let lines: Vec<&str> = source.lines().collect();
    let syntax = file_syntax("big.rs");
    let theme = Theme::cyber();
    let expected = highlight_all(syntax, &source);
    let mut cache = HighlightCache::default();
    for range in [5000..5040, 10..50, 4990..5010, 5900..5999, 0..3] {
        let got = cache.spans(syntax, &lines, range.clone(), str::to_string, &theme);
        assert_eq!(got, expected[range.clone()], "{range:?}");
    }
}

#[test]
fn cache_resets_for_a_different_source() {
    let theme = Theme::cyber();
    let mut cache = HighlightCache::default();
    let rust = file_syntax("a.rs");
    cache.spans(rust, &["/* open", "x"], 0..2, str::to_string, &theme);
    let got = cache.spans(rust, &["x", "y", "z"], 1..2, str::to_string, &theme);
    assert_eq!(got, vec![highlight_line(rust, "y", &theme)]);
}

#[test]
fn overlong_lines_are_plain() {
    let theme = Theme::cyber();
    let line = format!("let s = \"{}\";", "x".repeat(MAX_HIGHLIGHT_LINE_BYTES));
    let spans = highlight_line(file_syntax("a.rs"), &line, &theme);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content, line);
}
