//! Fonts and cell metrics.
//!
//! The screen is a grid of Geist Mono cells, like a terminal. Every cell
//! is 0.6 em wide (Geist Mono's advance) and 1.3 em tall (its ascender
//! plus descender). `AURA_DESKTOP_MONO_FONT` names another installed
//! monospace family, `AURA_DESKTOP_FONT_FILES` loads extra font files
//! (separated like `PATH`) and `AURA_DESKTOP_FONT_SIZE` sets the size.

use iced::font::{Family, Weight};
use iced::{Font, Size};
use std::borrow::Cow;
use std::sync::OnceLock;

/// Font size the window opens with.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;
pub const MIN_FONT_SIZE: f32 = 8.0;
pub const MAX_FONT_SIZE: f32 = 40.0;
/// Horizontal advance of one Geist Mono cell, in ems.
const ADVANCE_EM: f32 = 0.6;
/// Line height of one row, in ems.
const LINE_EM: f32 = 1.3;

const BUNDLED: [&[u8]; 2] = [
    include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Bold.ttf"),
];

fn family() -> &'static str {
    static FAMILY: OnceLock<&'static str> = OnceLock::new();
    FAMILY.get_or_init(|| match std::env::var("AURA_DESKTOP_MONO_FONT") {
        Ok(name) if !name.trim().is_empty() => Box::leak(name.trim().to_owned().into()),
        _ => "Geist Mono",
    })
}

/// Font files to register at startup: the bundled faces, then any listed
/// in `AURA_DESKTOP_FONT_FILES`.
pub fn font_files() -> Vec<Cow<'static, [u8]>> {
    let mut fonts: Vec<Cow<'static, [u8]>> = BUNDLED.iter().map(|b| Cow::Borrowed(*b)).collect();
    if let Some(paths) = std::env::var_os("AURA_DESKTOP_FONT_FILES") {
        for path in std::env::split_paths(&paths) {
            match std::fs::read(&path) {
                Ok(bytes) => fonts.push(Cow::Owned(bytes)),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "font file not loaded");
                }
            }
        }
    }
    fonts
}

/// Font size from `AURA_DESKTOP_FONT_SIZE`, else the default.
pub fn initial_font_size() -> f32 {
    std::env::var("AURA_DESKTOP_FONT_SIZE")
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .map_or(DEFAULT_FONT_SIZE, |s| s.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE))
}

pub fn mono() -> Font {
    Font {
        family: Family::Name(family()),
        ..Font::MONOSPACE
    }
}

pub fn mono_bold() -> Font {
    Font {
        weight: Weight::Bold,
        ..mono()
    }
}

/// Size of one cell at `font_size`. Rows are whole pixels so lines and
/// box borders land on the pixel grid.
pub fn cell_size(font_size: f32) -> Size {
    Size::new(font_size * ADVANCE_EM, (font_size * LINE_EM).round())
}
