//! Fonts and cell metrics.
//!
//! The screen is a grid of Geist Mono cells, like a terminal. Every cell
//! is 0.6 em wide (Geist Mono's advance) and 1.3 em tall (its ascender
//! plus descender). The chat's prose, what the user and the agent write
//! to each other, is set in Inter, proportionally; code, files, tool
//! output, thinking and the rest of the UI stay on the monospace grid.
//! `AURA_DESKTOP_MONO_FONT` and `AURA_DESKTOP_SANS_FONT` name other
//! installed families, `AURA_DESKTOP_FONT_FILES` loads extra font files
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
/// Size of the chat's prose relative to the grid's font size. Prose reads
/// a step larger than the code beside it, like body text in a chat app,
/// while still fitting the grid's rows.
const PROSE_EM: f32 = 1.1;

const BUNDLED: [&[u8]; 7] = [
    include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Bold.ttf"),
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-Bold.ttf"),
    include_bytes!("../assets/fonts/Inter-Italic.ttf"),
    include_bytes!("../assets/fonts/Inter-BoldItalic.ttf"),
];

/// The family named by the environment variable `var`, else `default`.
fn named(var: &str, default: &'static str) -> &'static str {
    match std::env::var(var) {
        Ok(name) if !name.trim().is_empty() => Box::leak(name.trim().to_owned().into()),
        _ => default,
    }
}

fn family() -> &'static str {
    static FAMILY: OnceLock<&'static str> = OnceLock::new();
    FAMILY.get_or_init(|| named("AURA_DESKTOP_MONO_FONT", "Geist Mono"))
}

fn sans_family() -> &'static str {
    static FAMILY: OnceLock<&'static str> = OnceLock::new();
    FAMILY.get_or_init(|| named("AURA_DESKTOP_SANS_FONT", "Inter"))
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

/// The proportional face the chat's prose is set in. Medium rather than
/// Regular: light text on a dark background renders thin without the
/// stem darkening other platforms apply, and Medium matches how body
/// text in chat apps reads.
pub fn sans() -> Font {
    Font {
        weight: Weight::Medium,
        ..Font::with_name(sans_family())
    }
}

pub fn sans_bold() -> Font {
    Font {
        weight: Weight::Bold,
        ..sans()
    }
}

/// Size of one cell at `font_size`. Rows are whole pixels so lines and
/// box borders land on the pixel grid.
pub fn cell_size(font_size: f32) -> Size {
    Size::new(font_size * ADVANCE_EM, (font_size * LINE_EM).round())
}

/// Size the chat's prose is set at when the grid is at `font_size`.
pub fn prose_size(font_size: f32) -> f32 {
    font_size * PROSE_EM
}
