//! Copying text to the system clipboard from inside the TUI.

use base64::Engine;
use std::io::{self, Write};
use std::process::{Command, Stdio};

/// Put `text` on the clipboard.
///
/// Writes an OSC 52 escape to `out`, which most terminals (and SSH
/// sessions) honour, and also pipes the text to the platform clipboard
/// tool, because VTE-based terminals such as GNOME Console ignore OSC 52.
///
/// # Errors
/// Returns the error from writing the escape to `out`.
pub(crate) fn copy(out: &mut impl Write, text: &str) -> io::Result<()> {
    out.write_all(osc52(text).as_bytes())?;
    out.flush()?;
    copy_with_system_tool(text);
    Ok(())
}

fn osc52(text: &str) -> String {
    format!(
        "\x1b]52;c;{}\x07",
        base64::engine::general_purpose::STANDARD.encode(text)
    )
}

#[cfg(target_os = "macos")]
fn copy_with_system_tool(text: &str) {
    let _ = pipe_to("pbcopy", &[], text.as_bytes());
}

#[cfg(windows)]
fn copy_with_system_tool(text: &str) {
    // `clip` reads UTF-16LE when the input starts with a BOM.
    let input: Vec<u8> = std::iter::once(0xFEFF_u16)
        .chain(text.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    let _ = pipe_to("clip", &[], &input);
}

#[cfg(not(any(target_os = "macos", windows)))]
fn copy_with_system_tool(text: &str) {
    let wayland: &[(&str, &[&str])] = &[("wl-copy", &[])];
    let x11: &[(&str, &[&str])] = &[
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    let mut tools = Vec::new();
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        tools.extend_from_slice(wayland);
    }
    if std::env::var_os("DISPLAY").is_some() {
        tools.extend_from_slice(x11);
    }
    for (program, args) in tools {
        if pipe_to(program, args, text.as_bytes()).is_ok() {
            return;
        }
    }
}

fn pipe_to(program: &str, args: &[&str], input: &[u8]) -> io::Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input)?;
    }
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_escape_carries_base64_text() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }
}
