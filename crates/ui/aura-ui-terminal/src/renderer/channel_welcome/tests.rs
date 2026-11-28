//! The banner font, and the banner and welcome fitting the chat.

use super::font::{self, Size};
use super::*;

/// Slugs of every shape a channel can have: short, digits, hyphens, and
/// the longest a slug may be (40 characters).
const SLUGS: &[&str] = &[
    "a",
    "aura",
    "zode-2",
    "web-site-relaunch",
    "q3-2026-planning-and-budget-review-hq",
    "abcdefghijklmnopqrstuvwxyz0123456789-xyz",
];

#[test]
fn the_font_draws_every_character_a_slug_can_hold() {
    let all = "abcdefghijklmnopqrstuvwxyz0123456789-";
    assert!(font::draws(all));
    assert!(font::draws("AURA"), "either case");
    assert!(!font::draws("a_b"));
    assert!(!font::draws(""));
    for size in [Size::Large, Size::Small] {
        let rows = font::render(all, size).unwrap();
        assert_eq!(rows.len(), size.rows());
        for row in &rows {
            assert_eq!(row.chars().count(), size.width(all), "{row:?}");
        }
    }
    assert_eq!(font::render("a#", Size::Large), None);
}

#[test]
fn a_large_banner_draws_square_pixels_two_cells_wide() {
    assert_eq!(
        font::render("a-", Size::Large).unwrap(),
        [
            "  ██          ",
            "██  ██        ",
            "██████  ██████",
            "██  ██        ",
            "██  ██        ",
        ]
    );
    assert_eq!(Size::Large.width("aura"), 30);
}

#[test]
fn a_small_banner_draws_two_pixel_rows_a_cell() {
    assert_eq!(
        font::render("a", Size::Small).unwrap(),
        ["▄▀▄", "█▀█", "▀ ▀"]
    );
    assert_eq!(Size::Small.width("aura"), 15);
}

#[test]
fn the_banner_takes_the_biggest_form_that_fits() {
    assert!(matches!(fit_banner("aura", 30, 5), Banner::Art(rows) if rows.len() == 5));
    // A column too narrow, or a row too few, for the large one.
    assert!(matches!(fit_banner("aura", 29, 5), Banner::Art(rows) if rows.len() == 3));
    assert!(matches!(fit_banner("aura", 80, 4), Banner::Art(rows) if rows.len() == 3));
    // Too narrow for one small line: broken after the hyphens.
    let Banner::Art(rows) = fit_banner("web-site-relaunch", 40, 20) else {
        panic!("expected a broken banner");
    };
    assert_eq!(rows.len(), 3 + 1 + 3, "{rows:#?}");
    assert_eq!(rows[3], "", "the lines a blank row apart");
    assert_eq!(rows[0].chars().count(), Size::Small.width("web-site-"));
    // Too narrow for any art, too short, or a character the font lacks.
    assert_eq!(
        fit_banner("aura", 14, 20),
        Banner::Plain("#aura".to_string())
    );
    assert_eq!(
        fit_banner("aura", 80, 2),
        Banner::Plain("#aura".to_string())
    );
    assert_eq!(fit_banner("a_b", 80, 20), Banner::Plain("#a_b".to_string()));
}

#[test]
fn a_slug_too_long_for_three_lines_is_plain_and_cut_to_fit() {
    let slug = SLUGS[5];
    assert!(matches!(fit_banner(slug, 60, 30), Banner::Plain(_)));
    assert_eq!(plain("relaunch", 6), "#rela…");
    assert_eq!(plain("relaunch", 1), "…");
    assert_eq!(plain("relaunch", 0), "");
}

#[test]
fn no_banner_is_ever_wider_or_taller_than_its_room() {
    for slug in SLUGS {
        for width in 0..=120 {
            for height in [0, 1, 2, 3, 4, 5, 7, 11, 30] {
                let banner = fit_banner(slug, width, height);
                let rows: Vec<String> = match banner {
                    Banner::Art(rows) => {
                        assert!(rows.len() <= height, "{slug} {width}x{height}");
                        rows
                    }
                    Banner::Plain(text) => vec![text],
                };
                for row in rows {
                    assert!(
                        row.chars().count() <= width,
                        "{slug} at {width}x{height}: {row:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_welcome_wraps_to_the_width_with_its_hint_last() {
    let welcome = ChannelWelcome {
        slug: "aura".to_string(),
        paragraphs: vec![
            "Welcome to #aura.".to_string(),
            "Tell me what you need.".into(),
        ],
        hint: "Mention a member with @CODE.".to_string(),
    };
    let lines = welcome_lines(&welcome, 12);
    assert!(
        lines.iter().all(|(line, _)| line.chars().count() <= 12),
        "{lines:?}"
    );
    assert_eq!(lines[0], ("Welcome to".to_string(), false));
    let (last, is_hint) = lines.last().unwrap();
    assert!(is_hint, "{last}");
    let blanks = lines.iter().filter(|(line, _)| line.is_empty()).count();
    assert_eq!(blanks, 2, "a blank row after each paragraph: {lines:?}");
}
