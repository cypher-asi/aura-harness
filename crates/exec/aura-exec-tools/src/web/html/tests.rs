use super::*;

fn md(html: &str) -> String {
    html_to_markdown(html, None).markdown
}

#[test]
fn headings_paragraphs_and_inline_formatting() {
    let out = md("<h1>Title</h1><p>Some <b>bold</b> and <em> soft </em> text.</p><h2>Next</h2>");
    assert_eq!(out, "# Title\n\nSome **bold** and *soft* text.\n\n## Next");
}

#[test]
fn strips_scripts_styles_and_navigation() {
    let out = md(
        r#"<html><head><title>T</title><style>p{color:red}</style></head>
            <body><nav><a href="/">Home</a></nav><script>var x = "<p>no</p>";</script>
            <p>Kept</p><footer>Copyright</footer><noscript>enable js</noscript></body></html>"#,
    );
    assert_eq!(out, "Kept");
}

#[test]
fn extracts_title_and_decodes_entities() {
    let page = html_to_markdown(
        "<title> A &amp; B </title><p>x &lt; y &#8212; &#x41; &nbsp;&unknown;</p>",
        None,
    );
    assert_eq!(page.title.as_deref(), Some("A & B"));
    assert_eq!(page.markdown, "x < y — A &unknown;");
}

#[test]
fn links_resolve_against_the_base_url() {
    let base = Url::parse("https://example.com/docs/intro").unwrap();
    let page = html_to_markdown(
        r##"<p>See <a href="guide">the guide</a>, <a href="https://other.org/x">other</a>,
            <a href="#top">top</a> and <a href="javascript:void(0)">js</a>.</p>"##,
        Some(&base),
    );
    assert_eq!(
            page.markdown,
            "See [the guide](https://example.com/docs/guide), [other](https://other.org/x), top and js."
        );
}

#[test]
fn nested_lists() {
    let out = md(
        "<ul><li>one</li><li>two<ol start=\"3\"><li>a</li><li>b</li></ol></li></ul><p>after</p>",
    );
    assert_eq!(out, "- one\n- two\n  3. a\n  4. b\n\nafter");
}

#[test]
fn code_blocks_keep_whitespace_and_language() {
    let out = md("<p>Run <code>cargo test</code>:</p><pre><code class=\"language-rust\">fn main() {\n    println!(\"&lt;hi&gt;\");\n}\n</code></pre><p>done</p>");
    assert_eq!(
        out,
        "Run `cargo test`:\n\n```rust\nfn main() {\n    println!(\"<hi>\");\n}\n```\n\ndone"
    );
}

#[test]
fn prefers_main_content_over_page_chrome() {
    let body = "word ".repeat(60);
    let html = format!(
        "<body><header><h1>Site</h1></header><div class=sidebar>Links</div>\
             <main><h1>Article</h1><p>{body}</p></main><div>Related</div></body>"
    );
    let out = md(&html);
    assert!(out.starts_with("# Article\n\nword word"), "{out}");
    assert!(!out.contains("Links") && !out.contains("Related") && !out.contains("Site"));
}

#[test]
fn falls_back_to_the_whole_page_when_main_is_nearly_empty() {
    let body = "word ".repeat(60);
    let out = md(&format!("<main></main><p>{body}</p>"));
    assert!(out.starts_with("word word"), "{out}");
}

#[test]
fn tables_and_blockquotes() {
    let out = md("<table><tr><th>Name</th><th>Kind</th></tr><tr><td>a</td><td>b</td></tr></table><blockquote><p>quoted</p><p>more</p></blockquote>");
    assert_eq!(
        out,
        "| Name | Kind |\n| --- | --- |\n| a | b |\n\n> quoted\n>\n> more"
    );
}

#[test]
fn tolerates_malformed_markup() {
    let out = md("<p>unclosed <b>bold <i>both</p><p>a < b and <br/>c</p><!-- note --><div");
    assert_eq!(out, "unclosed **bold *both***\n\na < b and\nc");
}

#[test]
fn snippet_text_drops_tags() {
    assert_eq!(
        html_to_text("The <strong>Rust</strong>  &amp; <em>Cargo</em>\nbook"),
        "The Rust & Cargo book"
    );
}
