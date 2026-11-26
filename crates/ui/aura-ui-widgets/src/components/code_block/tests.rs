use super::*;
use crate::themes::CYAN;

#[test]
fn test_code_block_creation() {
    let block = CodeBlock::new("rust", "fn main() {\n    println!(\"Hello\");\n}");
    assert_eq!(block.language, "rust");
    assert_eq!(block.lines.len(), 3);
}

#[test]
fn json_keys_are_plain_and_values_are_strings() {
    let theme = Theme::cyber();
    // Keys are only keys inside an object, so highlight with context.
    let syntax = super::syntax_for_token("json").unwrap();
    let mut highlighter = crate::components::syntax::LineHighlighter::new(syntax);
    highlighter.spans("{", &theme);
    let spans = highlighter.spans(r#"  "tool": "run_command","#, &theme);
    let color_of = |text: &str| {
        spans
            .iter()
            .find(|s| s.content == text)
            .and_then(|s| s.style.fg)
    };
    assert_eq!(color_of(r#""tool""#), Some(theme.colors.foreground));
    assert_eq!(color_of(r#""run_command""#), Some(CYAN));
}

#[test]
fn code_block_carries_state_across_lines() {
    let theme = Theme::cyber();
    let block = CodeBlock::new("rust", "/* open\n   inside */\nfn x() {}");
    let lines = block.render(&theme, 80);
    let inside = lines[2]
        .spans
        .iter()
        .find(|s| s.content.contains("inside"))
        .unwrap();
    assert_eq!(inside.style.fg, Some(crate::themes::GRAY));
}

fn text_of(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Every row is as wide as the top border and no wider than `max`,
/// and every body row ends on the right border.
fn assert_boxed(lines: &[Line<'_>], max: usize) {
    let top = display_width(&text_of(&lines[0]));
    assert!(top <= max, "box {top} wider than {max}");
    for line in lines {
        let text = text_of(line);
        assert_eq!(display_width(&text), top, "{text:?}");
    }
    for line in &lines[1..lines.len() - 1] {
        let text = text_of(line);
        assert!(
            text.starts_with("│ ") && (text.ends_with(" │") || text.ends_with("\u{a0}│")),
            "{text:?}"
        );
    }
}

#[test]
fn long_lines_wrap_inside_the_box() {
    let theme = Theme::cyber();
    let code = "DAYTONA_API_KEY=dtn_x\n\
                # DAYTONA_ORGANIZATION_ID=     # only if the key can reach several organizations\n\
                # AURA_CLOUD_CPU=4";
    for language in ["dotenv", "bash", ""] {
        let lines = CodeBlock::new(language, code).render(&theme, 40);
        assert_boxed(&lines, 40);
        let body: String = lines[1..lines.len() - 1]
            .iter()
            .map(|line| {
                let text = text_of(line);
                text.strip_prefix("│ ")
                    .and_then(|t| t.strip_suffix('│'))
                    .unwrap()
                    .trim_end_matches([' ', '\u{a0}'])
                    .to_owned()
            })
            .collect();
        assert!(body.contains("several organizations"), "{body:?}");
        assert!(lines.len() > 5, "the long line wrapped");
    }
}

#[test]
fn wide_characters_and_tabs_stay_inside_the_box() {
    let theme = Theme::cyber();
    let lines = CodeBlock::new("", "\tlet s = \"日本語のテキストです\";").render(&theme, 16);
    assert_boxed(&lines, 16);
}

#[test]
fn short_blocks_fit_their_text_and_narrow_panes_drop_the_label() {
    let theme = Theme::cyber();
    let lines = CodeBlock::new("rust", "let x = 1;").render(&theme, 80);
    assert_boxed(&lines, 80);
    assert_eq!(text_of(&lines[1]), "│ let x = 1; │");
    let lines = CodeBlock::new("javascript", "x").render(&theme, 8);
    assert_boxed(&lines, 8);
}

/// `lines` as screen text, each indented by `indent` like chat rows.
fn screen_rows(lines: &[Line<'_>], indent: &str) -> Vec<String> {
    lines
        .iter()
        .map(|line| format!("{indent}{}", text_of(line)).trim_end().to_owned())
        .collect()
}

#[test]
fn copying_a_wrapped_command_drops_the_frame_and_joins_the_line() {
    let theme = Theme::cyber();
    let command = "cd ~/code && rm -rf wilder-simm/target textcat/target zrm/backend/target \
                   the-grid/target machina-2/backend/target zero-os/target";
    let code = format!("{command}\necho done  \n\n    indented");
    let lines = CodeBlock::new("bash", &code).render(&theme, 40);
    assert!(lines.len() > 6, "the command wrapped");
    let copied = unframe_code(&screen_rows(&lines, "  "));
    assert_eq!(copied, [command, "echo done", "", "    indented"]);
}

#[test]
fn a_selection_cut_short_still_loses_the_frame() {
    let theme = Theme::cyber();
    let lines = CodeBlock::new("bash", "first line\nsecond line\nthird line").render(&theme, 80);
    let mut rows = screen_rows(&lines[1..4], "");
    // Started inside the first code row, ended inside the last.
    rows[0] = rows[0]["│ fir".len()..].to_owned();
    rows[2] = rows[2][.."│ third".len()].to_owned();
    assert_eq!(unframe_code(&rows), ["st line", "second line", "third"]);
}

#[test]
fn rows_outside_a_frame_are_kept_as_they_are() {
    let rows: Vec<String> = ["│ a quote", "│ another │ thing", "plain", "│ tree"]
        .map(str::to_owned)
        .to_vec();
    let copied = unframe_code(&rows);
    assert_eq!(copied[0], "│ a quote");
    assert_eq!(copied[2], "plain");
    assert_eq!(copied[3], "│ tree", "a last row needs the frame above it");
}

#[test]
fn one_or_two_rows_cut_short_lose_the_frame_and_join() {
    let theme = Theme::cyber();
    let command = "cd ~/code && rm -rf wilder-simm/target textcat/target";
    let lines = CodeBlock::new("bash", command).render(&theme, 40);
    assert_eq!(lines.len(), 4, "the command takes two rows");
    let mut rows = screen_rows(&lines[1..3], "  ");
    // Dragged from its first character to its last.
    rows[0] = rows[0].trim_start()["│ ".len()..].to_owned();
    let end = rows[1].find("target").unwrap() + "target".len();
    rows[1].truncate(end);
    assert_eq!(unframe_code(&rows), [command]);

    // One row, dragged from the code past its end.
    let lines = CodeBlock::new("bash", "echo hi").render(&theme, 40);
    let row = text_of(&lines[1])["│ ".len()..].to_owned();
    assert_eq!(unframe_code(&[row]), ["echo hi"]);
}

#[test]
fn tables_keep_their_borders() {
    let table: Vec<String> = [
        "┌──────┬─────┐",
        "│ Name │ Qty │",
        "├──────┼─────┤",
        "│ nuts │ 4   │",
        "└──────┴─────┘",
    ]
    .map(str::to_owned)
    .to_vec();
    assert_eq!(unframe_code(&table), table);
    assert_eq!(unframe_code(&table[1..4]), table[1..4]);
    // Next to a code block, still the table's own.
    let theme = Theme::cyber();
    let mut rows = screen_rows(&CodeBlock::new("bash", "ls").render(&theme, 40), "");
    rows.extend(table.iter().cloned());
    assert_eq!(unframe_code(&rows)[1..], table[..]);
}

#[test]
fn a_block_in_a_gutter_loses_the_gutter_and_the_frame() {
    let theme = Theme::cyber();
    let lines = CodeBlock::new("rust", "let x = 1;\nlet y = 2;").render(&theme, 40);
    let rows = screen_rows(&lines, "  │ ");
    assert_eq!(unframe_code(&rows), ["let x = 1;", "let y = 2;"]);
    // Started below the top border.
    assert_eq!(unframe_code(&rows[2..]), ["let y = 2;"]);
}

/// The columns `range` of `line`'s text, for rows of one-column
/// characters.
fn columns(line: &Line<'_>, range: Range<usize>) -> String {
    text_of(line)
        .chars()
        .skip(range.start)
        .take(range.len())
        .collect()
}

#[test]
fn copy_button_sits_in_the_top_border_of_a_widened_short_block() {
    let theme = Theme::cyber();
    let block = CodeBlock::new("bash", "ls");
    let plain = block.render(&theme, 80);
    let (lines, button) = block.render_copyable(&theme, 80);
    assert_boxed(&lines, 80);
    let button = button.expect("the button fits in 80 columns");
    assert_eq!(columns(&lines[0], button.clone()), COPY_BUTTON);
    let top = text_of(&lines[0]);
    assert!(top.starts_with("╭─ Shell ─"), "{top:?}");
    assert!(top.ends_with(" ⧉ Copy ─╮"), "{top:?}");
    assert_eq!(button.end + 2, display_width(&top), "the corner follows it");
    assert!(
        display_width(&top) > display_width(&text_of(&plain[0])),
        "the block widened to fit the button"
    );
    assert_eq!(text_of(&lines[1]).trim_end_matches(['│', ' ']), "│ ls");
}

#[test]
fn copy_button_is_left_out_when_the_box_is_too_narrow() {
    let theme = Theme::cyber();
    let (lines, button) = CodeBlock::new("bash", "ls").render_copyable(&theme, 16);
    assert_boxed(&lines, 16);
    assert_eq!(button, None);
    assert!(
        !text_of(&lines[0]).contains("Copy"),
        "{:?}",
        text_of(&lines[0])
    );
}

#[test]
fn plain_render_never_shows_the_copy_button() {
    let theme = Theme::cyber();
    for width in [16, 40, 80, 200] {
        let lines = CodeBlock::new("bash", "echo hi\nls").render(&theme, width);
        assert!(!text_of(&lines[0]).contains("Copy"), "{width}");
    }
}

#[test]
fn selecting_a_copyable_block_copies_just_its_code() {
    let theme = Theme::cyber();
    let (lines, button) = CodeBlock::new("bash", "echo hi\nls").render_copyable(&theme, 80);
    assert!(button.is_some());
    let copied = unframe_code(&screen_rows(&lines, "  "));
    assert_eq!(copied, ["echo hi", "ls"]);
}

#[test]
fn test_language_labels() {
    assert_eq!(get_language_label("rust"), "Rust");
    assert_eq!(get_language_label("py"), "Python");
    assert_eq!(get_language_label("js"), "JavaScript");
    assert_eq!(get_language_label("vue"), "Vue Component");
    assert_eq!(get_language_label(""), "Code");
}
