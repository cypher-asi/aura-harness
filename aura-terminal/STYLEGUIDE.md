# AURA Terminal Style Guide

This document defines the approved color palette and styling conventions for the AURA CLI terminal UI.

## Approved Color Palette

Only these colors should be used in the terminal UI:

| Color       | Hex Code  | RGB             | Rust Constant | Usage                              |
|-------------|-----------|-----------------|---------------|------------------------------------|
| Cyan/Green  | `#01f4cb` | `(1, 244, 203)` | `CYAN`        | Success states, neon accents       |
| Blue        | `#01a4f4` | `(1, 164, 244)` | `BLUE`        | Primary accent, info, provisioning |
| Purple      | `#cb01f4` | `(203, 1, 244)` | `PURPLE`      | Pending states, secondary accent   |
| Red         | `#f4012a` | `(244, 1, 42)`  | `RED`         | Errors, danger                     |
| White       | `#ffffff` | `(255, 255, 255)` | `WHITE`     | Primary text                       |
| Gray        | `#888888` | `(136, 136, 136)` | `GRAY`      | Muted text, secondary info         |
| Black       | `#0d0d0d` | `(13, 13, 13)`  | `BLACK`       | Background                         |

## Semantic Color Mapping

Use theme colors semantically via `theme.colors.*`:

| Theme Field       | Color   | When to Use                                    |
|-------------------|---------|------------------------------------------------|
| `background`      | Black   | Panel backgrounds, main background             |
| `foreground`      | White   | Primary text, titles, important content        |
| `primary`         | Blue    | Primary accents, active states, focus borders  |
| `secondary`       | Purple  | Secondary accents, tool calls, highlights      |
| `success`         | Cyan    | Success messages, completed states, checkmarks |
| `warning`         | Blue    | Info messages, warnings, provisioning states   |
| `error`           | Red     | Error messages, failed states, danger actions  |
| `pending`         | Purple  | Pending states, in-progress, spinners          |
| `muted`           | Gray    | Secondary text, timestamps, less important info|

## Usage Guidelines

### Text Colors

- **Primary text**: Use `theme.colors.foreground` (white)
- **Secondary/muted text**: Use `theme.colors.muted` (gray)
- **Emphasized text**: Use accent colors sparingly

### Status Indicators

- **Success (✓)**: Use `theme.colors.success` (cyan)
- **Error (✗)**: Use `theme.colors.error` (red)
- **Pending (◌)**: Use `theme.colors.pending` (purple)
- **Neutral (·)**: Use `theme.colors.muted` (gray)

### Borders and Panels

- **Focused panel**: Use `theme.colors.primary` (blue) for border
- **Unfocused panel**: Use `theme.colors.muted` (gray) for border
- **Error highlight**: Use `theme.colors.error` (red) for border

### Messages

- **User messages**: Gray text with white nickname
- **Assistant messages**: White text with blue accent
- **System messages**: Muted gray text
- **Error messages**: Red text or red icon

### Interactive Elements

- **Selected item**: Blue background or blue text
- **Hover state**: Purple highlight
- **Active/pressed**: Cyan accent

## Code Examples

### Using Theme Colors

```rust
use crate::themes::Theme;

fn render_status(theme: &Theme, is_success: bool) -> Style {
    if is_success {
        Style::default().fg(theme.colors.success)
    } else {
        Style::default().fg(theme.colors.error)
    }
}
```

### Using Color Constants Directly

```rust
use crate::themes::{CYAN, BLUE, PURPLE, RED, WHITE, GRAY, BLACK};

// Only use constants when theme is not available
let success_style = Style::default().fg(CYAN);
```

### Status Indicator Pattern

```rust
let (icon, color) = match status {
    Status::Ok => ("✓", theme.colors.success),
    Status::Error => ("✗", theme.colors.error),
    Status::Pending => ("◌", theme.colors.pending),
    Status::None => ("·", theme.colors.muted),
};
```

## Do's and Don'ts

### Do

- Use semantic theme colors (`theme.colors.success`, etc.)
- Keep text readable with sufficient contrast
- Use cyan for positive/success feedback
- Use red sparingly, only for actual errors
- Use purple for in-progress/pending states

### Don't

- Use arbitrary RGB values not in the palette
- Use yellow, orange, pink, or other unapproved colors
- Mix too many accent colors in one view
- Use color as the only indicator (add icons too)
- Use bright colors for large text blocks

## Migration Notes

When updating existing code:

1. Replace `Color::Rgb(...)` with theme colors or constants
2. Replace `Color::Yellow`, `Color::Magenta`, etc. with approved colors
3. Use `theme.colors.pending` for pending states (was often warning)
4. Use `theme.colors.error` (red) instead of hot pink for errors
