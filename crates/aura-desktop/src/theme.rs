//! Colors, fonts and widget styles.
//!
//! Colors follow the cyber palette of the terminal UI
//! (`aura-terminal/STYLEGUIDE.md`). Text is set in Geist and code in
//! Geist Mono, both bundled; `AURA_DESKTOP_FONT` / `AURA_DESKTOP_MONO_FONT`
//! name other installed families and `AURA_DESKTOP_FONT_FILES` loads extra
//! font files (separated like `PATH`).

use iced::font::{Family, Weight};
use iced::theme::Palette;
use iced::widget::{button, container, markdown, text_editor, text_input};
use iced::{border, Background, Border, Color, Font, Pixels, Shadow, Theme};
use std::borrow::Cow;
use std::sync::OnceLock;

pub const CYAN: Color = Color::from_rgb8(1, 244, 203);
pub const BLUE: Color = Color::from_rgb8(1, 164, 244);
pub const PURPLE: Color = Color::from_rgb8(203, 1, 244);
pub const RED: Color = Color::from_rgb8(244, 1, 42);
pub const WHITE: Color = Color::WHITE;
pub const TEXT: Color = Color::from_rgb8(212, 212, 212);
pub const MUTED: Color = Color::from_rgb8(136, 136, 136);
pub const FAINT: Color = Color::from_rgb8(90, 90, 96);
pub const BG: Color = Color::from_rgb8(13, 13, 13);
pub const PANEL: Color = Color::from_rgb8(18, 18, 20);
pub const CELL: Color = Color::from_rgb8(30, 30, 30);
pub const HOVER: Color = Color::from_rgb8(42, 42, 42);
pub const LINE: Color = Color::from_rgb8(36, 36, 40);

/// Body text size.
pub const TEXT_SIZE: f32 = 14.0;
/// Code and command output size.
pub const MONO_SIZE: f32 = 13.0;
/// Labels, timestamps and other secondary text.
pub const SMALL_SIZE: f32 = 12.0;
/// Horizontal advance of one Geist Mono cell, in ems.
pub const MONO_ADVANCE_EM: f32 = 0.6;

const BUNDLED: [&[u8]; 8] = [
    include_bytes!("../assets/fonts/Geist-Regular.ttf"),
    include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Geist-Bold.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Medium.ttf"),
    include_bytes!("../assets/fonts/GeistMono-SemiBold.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Bold.ttf"),
];

struct Families {
    ui: &'static str,
    mono: &'static str,
}

fn families() -> &'static Families {
    static FAMILIES: OnceLock<Families> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        let pick = |var: &str, default: &'static str| -> &'static str {
            match std::env::var(var) {
                Ok(name) if !name.trim().is_empty() => Box::leak(name.trim().to_owned().into()),
                _ => default,
            }
        };
        Families {
            ui: pick("AURA_DESKTOP_FONT", "Geist"),
            mono: pick("AURA_DESKTOP_MONO_FONT", "Geist Mono"),
        }
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

/// Proportional UI font.
pub fn ui() -> Font {
    Font {
        family: Family::Name(families().ui),
        ..Font::DEFAULT
    }
}

pub fn ui_medium() -> Font {
    Font {
        weight: Weight::Medium,
        ..ui()
    }
}

pub fn ui_semibold() -> Font {
    Font {
        weight: Weight::Semibold,
        ..ui()
    }
}

/// Monospace font for code, commands and record fields.
pub fn mono() -> Font {
    Font {
        family: Family::Name(families().mono),
        ..Font::MONOSPACE
    }
}

pub fn mono_medium() -> Font {
    Font {
        weight: Weight::Medium,
        ..mono()
    }
}

pub fn theme() -> Theme {
    Theme::custom(
        "AURA",
        Palette {
            background: BG,
            text: TEXT,
            primary: BLUE,
            success: CYAN,
            warning: PURPLE,
            danger: RED,
        },
    )
}

pub fn markdown_settings() -> markdown::Settings {
    let style = markdown::Style {
        font: ui(),
        inline_code_highlight: markdown::Highlight {
            background: Background::Color(CELL),
            border: border::rounded(4),
        },
        inline_code_padding: iced::Padding::from([0, 3]),
        inline_code_color: CYAN,
        inline_code_font: mono(),
        code_block_font: mono(),
        link_color: BLUE,
    };
    let mut settings = markdown::Settings::with_text_size(TEXT_SIZE, style);
    settings.h1_size = Pixels(22.0);
    settings.h2_size = Pixels(19.0);
    settings.h3_size = Pixels(16.0);
    settings.h4_size = Pixels(TEXT_SIZE);
    settings.code_size = Pixels(MONO_SIZE);
    settings.spacing = Pixels(10.0);
    settings
}

// ---------------------------------------------------------------------------
// Containers
// ---------------------------------------------------------------------------

pub fn app_background(_: &Theme) -> container::Style {
    container::Style {
        background: Some(BG.into()),
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}

pub fn panel(_: &Theme) -> container::Style {
    container::Style {
        background: Some(PANEL.into()),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn bar(_: &Theme) -> container::Style {
    container::Style {
        background: Some(PANEL.into()),
        ..container::Style::default()
    }
}

pub fn card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(CELL.into()),
        border: border::rounded(8),
        ..container::Style::default()
    }
}

pub fn code(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb8(10, 10, 12).into()),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

/// A line of a diff, marked by a colored bar in the gutter.
pub fn diff_line(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Color { a: 0.08, ..color }.into()),
        border: Border {
            color: Color { a: 0.0, ..color },
            width: 0.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn modal(_: &Theme) -> container::Style {
    container::Style {
        background: Some(PANEL.into()),
        border: Border {
            color: Color { a: 0.5, ..BLUE },
            width: 1.0,
            radius: 12.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba8(0, 0, 0, 0.6),
            offset: iced::Vector::new(0.0, 12.0),
            blur_radius: 40.0,
        },
        ..container::Style::default()
    }
}

pub fn scrim(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba8(0, 0, 0, 0.55).into()),
        ..container::Style::default()
    }
}

pub fn badge(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Color { a: 0.14, ..color }.into()),
        text_color: Some(color),
        border: border::rounded(5),
        ..container::Style::default()
    }
}

pub fn input_shell(focused_color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(CELL.into()),
        border: Border {
            color: Color {
                a: 0.55,
                ..focused_color
            },
            width: 1.0,
            radius: 10.0.into(),
        },
        ..container::Style::default()
    }
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// Flat row or link: no chrome until hovered.
pub fn ghost(_: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => Some(HOVER.into()),
        _ => None,
    };
    button::Style {
        background,
        text_color: TEXT,
        border: border::rounded(6),
        ..button::Style::default()
    }
}

/// Row in a list; `selected` keeps the highlight.
pub fn list_row(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let background = match (selected, status) {
            (true, _) => Some(Color { a: 0.16, ..BLUE }.into()),
            (false, button::Status::Hovered | button::Status::Pressed) => Some(HOVER.into()),
            _ => None,
        };
        button::Style {
            background,
            text_color: if selected { WHITE } else { TEXT },
            border: border::rounded(6),
            ..button::Style::default()
        }
    }
}

pub fn tab(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: if active {
                Some(CELL.into())
            } else if hovered {
                Some(Color { a: 0.5, ..CELL }.into())
            } else {
                None
            },
            text_color: if active { WHITE } else { MUTED },
            border: border::rounded(6),
            ..button::Style::default()
        }
    }
}

pub fn accent(color: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let alpha = match status {
            button::Status::Hovered => 0.32,
            button::Status::Pressed => 0.42,
            button::Status::Disabled => 0.08,
            button::Status::Active => 0.2,
        };
        button::Style {
            background: Some(Color { a: alpha, ..color }.into()),
            text_color: if status == button::Status::Disabled {
                MUTED
            } else {
                color
            },
            border: Border {
                color: Color { a: 0.5, ..color },
                width: 1.0,
                radius: 7.0.into(),
            },
            ..button::Style::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

pub fn editor(_: &Theme, _status: text_editor::Status) -> text_editor::Style {
    text_editor::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        placeholder: FAINT,
        value: WHITE,
        selection: Color { a: 0.35, ..BLUE },
    }
}

pub fn field(_: &Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(CELL),
        border: Border {
            color: if focused { BLUE } else { LINE },
            width: 1.0,
            radius: 7.0.into(),
        },
        icon: MUTED,
        placeholder: FAINT,
        value: WHITE,
        selection: Color { a: 0.35, ..BLUE },
    }
}
