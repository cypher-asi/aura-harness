use super::font::{self, Size};
use super::*;
use crate::events::AgentSummary;

/// Every character a slug may hold.
const SLUG_CHARS: &str = "abcdefghijklmnopqrstuvwxyz0123456789-";

fn channel(slug: &str, title: &str, description: &str) -> ChannelSummary {
    ChannelSummary {
        id: "c1".to_string(),
        slug: slug.to_string(),
        title: title.to_string(),
        description: description.to_string(),
        coordinator_id: "co".to_string(),
        members: vec![AgentSummary {
            id: "co".to_string(),
            code: Some("CO2".to_string()),
            name: "Coordinator".to_string(),
            ..AgentSummary::default()
        }],
        ..ChannelSummary::default()
    }
}

/// Columns of each of `banner`'s rows, and how many rows it has.
fn extent(banner: &Banner) -> (usize, usize) {
    let rows = match banner {
        Banner::Art { rows, .. } => rows.clone(),
        Banner::Plain(text) => vec![text.clone()],
    };
    let widest = rows
        .iter()
        .map(|row| crate::renderer::text::display_width(row))
        .max()
        .unwrap_or(0);
    (widest, rows.len())
}

#[test]
fn test_every_slug_character_has_a_five_row_glyph_of_one_width() {
    for c in SLUG_CHARS.chars() {
        let glyph = font::glyph(c).unwrap_or_else(|| panic!("no glyph for {c:?}"));
        let width = glyph[0].len();
        assert!((1..=5).contains(&width), "{c:?} is {width} wide");
        for row in glyph {
            assert_eq!(row.len(), width, "{c:?}: {glyph:?}");
            assert!(row.chars().all(|p| p == '#' || p == ' '), "{c:?}");
        }
        assert!(glyph.iter().any(|row| row.contains('#')), "{c:?} is blank");
    }
    assert_eq!(
        font::glyph('A'),
        font::glyph('a'),
        "uppercase reads as lowercase"
    );
    assert!(font::glyph('_').is_none());
}

#[test]
fn test_drawn_rows_are_as_wide_and_tall_as_the_size_says() {
    for text in ["aura", "a", "release-2024", SLUG_CHARS] {
        for size in Size::ALL {
            let rows = font::draw(text, size).unwrap();
            let width = font::text_width(text, size).unwrap();
            assert_eq!(rows.len(), size.rows(), "{text} at {size:?}");
            for row in &rows {
                assert_eq!(row.chars().count(), width, "{text} at {size:?}: {row:?}");
            }
        }
    }
    // Four 4-pixel glyphs with a pixel between each two.
    assert_eq!(font::text_width("aura", Size::Small), Some(19));
    assert_eq!(font::text_width("aura", Size::Large), Some(38));
}

#[test]
fn test_letters_are_drawn_in_block_characters() {
    let large = font::draw("a", Size::Large).unwrap();
    assert_eq!(large[0], "  ████  ");
    assert_eq!(large[2], "████████");
    let small = font::draw("a", Size::Small).unwrap();
    // Rows 0-1, 2-3, then 4 over nothing.
    assert_eq!(small, ["▄▀▀▄", "█▀▀█", "▀  ▀"]);
}

#[test]
fn test_banner_takes_the_largest_size_that_fits() {
    let size_at = |width| match fit_banner("aura", width, 12) {
        Some(Banner::Art { size, .. }) => Some(size),
        _ => None,
    };
    assert_eq!(size_at(60), Some(Size::Large));
    assert_eq!(size_at(38), Some(Size::Large));
    assert_eq!(size_at(37), Some(Size::Small));
    assert_eq!(size_at(19), Some(Size::Small));
    assert_eq!(size_at(18), None);
    assert_eq!(
        fit_banner("aura", 18, 12),
        Some(Banner::Plain("#aura".to_string()))
    );
}

#[test]
fn test_banner_shrinks_to_the_rows_it_is_given() {
    let size_in = |rows| match fit_banner("aura", 80, rows) {
        Some(Banner::Art { size, .. }) => Some(size),
        _ => None,
    };
    assert_eq!(size_in(5), Some(Size::Large));
    assert_eq!(size_in(4), Some(Size::Small));
    assert_eq!(size_in(3), Some(Size::Small));
    assert_eq!(size_in(2), None, "plain text");
    assert_eq!(fit_banner("aura", 80, 0), None);
    assert_eq!(fit_banner("aura", 0, 10), None);
}

#[test]
fn test_hyphenated_slug_wraps_after_its_hyphens() {
    // "release-notes-2024" is 82 columns small, "release-" 38 and
    // "notes-2024" 43: at 40 columns each part takes a line.
    let Some(Banner::Art { size, rows }) = fit_banner("release-notes-2024", 40, 20) else {
        panic!("no art");
    };
    assert_eq!(size, Size::Small);
    // Three lines of three rows, a blank row between each two.
    assert_eq!(rows.len(), 11);
    assert!(rows[3].is_empty() && rows[7].is_empty(), "{rows:?}");
    let expected = font::draw("release-", Size::Small).unwrap();
    assert_eq!(rows[..3], expected[..]);
    // Too short for three lines: plain text.
    assert_eq!(
        fit_banner("release-notes-2024", 40, 10),
        Some(Banner::Plain("#release-notes-2024".to_string()))
    );
}

#[test]
fn test_long_or_unknown_slugs_fall_back_to_plain_text() {
    let long = "a".repeat(40);
    let Some(Banner::Plain(text)) = fit_banner(&long, 30, 20) else {
        panic!("a 40-letter slug is never art at 30 columns");
    };
    assert_eq!(text.chars().count(), 30);
    assert!(text.starts_with("#aaa") && text.ends_with('…'), "{text}");
    assert_eq!(
        fit_banner("aura", 1, 5),
        Some(Banner::Plain("…".to_string()))
    );
    assert_eq!(
        fit_banner("a_b", 80, 5),
        Some(Banner::Plain("#a_b".to_string()))
    );
}

#[test]
fn test_banner_never_exceeds_its_width_or_rows() {
    let slugs = [
        "aura",
        "x",
        "-",
        "release-notes-2024",
        "a-b-c-d-e-f-g-h",
        "mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm",
        "2024",
    ];
    for slug in slugs {
        for width in 1..=120 {
            for rows in 1..=12 {
                let banner = fit_banner(slug, width, rows)
                    .unwrap_or_else(|| panic!("{slug} at {width}x{rows}: nothing"));
                let (wide, tall) = extent(&banner);
                assert!(wide <= width, "{slug} at {width}x{rows}: {wide} wide");
                assert!(tall <= rows, "{slug} at {width}x{rows}: {tall} tall");
            }
        }
    }
}

#[test]
fn test_welcome_says_what_the_channel_is_for() {
    let said = welcome(&channel("aura", "Aura", "agentic harness for swarms"));
    assert_eq!(said.author, "CO2");
    assert_eq!(
        said.paragraphs,
        [
            "Welcome to #aura. agentic harness for swarms.".to_string(),
            GET_STARTED.to_string()
        ]
    );
    // A description already ending a sentence, over lines, is one line.
    let said = welcome(&channel("aura", "", "Ship\n  it!"));
    assert_eq!(said.paragraphs[0], "Welcome to #aura. Ship it!");
}

#[test]
fn test_welcome_falls_back_to_the_title_then_to_nothing() {
    let said = welcome(&channel("zode", "Zode release", ""));
    assert_eq!(said.paragraphs[0], "Welcome to #zode. Zode release.");
    let said = welcome(&channel("zode", "zode", " "));
    assert_eq!(said.paragraphs[0], "Welcome to #zode.");
    assert!(GET_STARTED.contains("hand it to an agent"));
}

#[test]
fn test_welcome_label_is_the_coordinators_code_else_its_name() {
    let mut named = channel("aura", "", "");
    named.members[0].code = None;
    assert_eq!(welcome(&named).author, "Coordinator");
    named.members[0].name = "Lead".to_string();
    assert_eq!(welcome(&named).author, "Lead");
    named.members.clear();
    assert_eq!(welcome(&named).author, FALLBACK_AUTHOR);
}

#[test]
fn test_welcome_text_wraps_to_the_width() {
    let said = welcome(&channel(
        "aura",
        "",
        "agentic harness for swarm super intelligence",
    ));
    let theme = Theme::cyber();
    for width in [1, 10, 24, 80] {
        let lines = text_lines(&said, width, &theme);
        assert_eq!(lines[0].to_string(), "CO2");
        for line in &lines[1..] {
            assert!(line.width() <= width, "{width}: {line}");
        }
    }
    assert!(text_lines(&said, 0, &theme).is_empty());
}
