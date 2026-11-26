//! Copying text to the system clipboard from inside the TUI, and reading
//! an image (or text) back for a paste.

use base64::Engine;
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

/// How long a clipboard tool may take to answer. The UI waits for it, so
/// a tool stuck on a dead display server must not freeze the terminal.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Most bytes read from a clipboard tool. Anything larger is far past the
/// image size limit, so there is no point holding it in memory.
const MAX_READ_BYTES: u64 = 32 * 1024 * 1024;

/// Image types asked for, in order of preference.
const IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// What the clipboard holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clipboard {
    /// Image bytes, as the clipboard tool gave them
    Image(Vec<u8>),
    /// Text
    Text(String),
    /// Nothing pasteable
    Empty,
}

/// Read the clipboard, preferring an image over text.
///
/// # Errors
/// Returns an error when no clipboard tool is available or it fails.
pub fn read() -> io::Result<Clipboard> {
    if let Some(image) = read_image()? {
        return Ok(Clipboard::Image(image));
    }
    Ok(match read_text()? {
        Some(text) if !text.is_empty() => Clipboard::Text(text),
        _ => Clipboard::Empty,
    })
}

#[cfg(not(any(target_os = "macos", windows)))]
fn read_image() -> io::Result<Option<Vec<u8>>> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();
    let mut tool_ran = false;
    if wayland {
        match capture("wl-paste", &["--list-types"]) {
            Ok(types) => {
                return match preferred_image_type(&String::from_utf8_lossy(&types)) {
                    Some(kind) => capture("wl-paste", &["--no-newline", "--type", kind]).map(Some),
                    None => Ok(None),
                };
            }
            // `wl-paste` fails when the clipboard is empty.
            Err(e) => tool_ran |= e.kind() != io::ErrorKind::NotFound,
        }
    }
    if x11 {
        match capture("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"]) {
            Ok(types) => {
                return match preferred_image_type(&String::from_utf8_lossy(&types)) {
                    Some(kind) => {
                        capture("xclip", &["-selection", "clipboard", "-t", kind, "-o"]).map(Some)
                    }
                    None => Ok(None),
                };
            }
            Err(e) => tool_ran |= e.kind() != io::ErrorKind::NotFound,
        }
    }
    if tool_ran {
        return Ok(None);
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no clipboard tool found (install wl-clipboard or xclip)",
    ))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn read_text() -> io::Result<Option<String>> {
    let mut tools: Vec<(&str, &[&str])> = Vec::new();
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        tools.push(("wl-paste", &["--no-newline"]));
    }
    if std::env::var_os("DISPLAY").is_some() {
        tools.push(("xclip", &["-selection", "clipboard", "-o"]));
    }
    first_text(&tools)
}

#[cfg(target_os = "macos")]
fn read_image() -> io::Result<Option<Vec<u8>>> {
    // AppleScript prints PNG data as `«data PNGf89504E47…»`.
    let script = "the clipboard as «class PNGf»";
    match capture("osascript", &["-e", script]) {
        Ok(out) => Ok(decode_applescript_data(&String::from_utf8_lossy(&out))),
        Err(_) => Ok(None),
    }
}

#[cfg(target_os = "macos")]
fn read_text() -> io::Result<Option<String>> {
    first_text(&[("pbpaste", &[])])
}

#[cfg(windows)]
fn read_image() -> io::Result<Option<Vec<u8>>> {
    let script = "Add-Type -AssemblyName System.Windows.Forms; \
        $img = [Windows.Forms.Clipboard]::GetImage(); \
        if ($img) { $ms = New-Object IO.MemoryStream; \
        $img.Save($ms, [Drawing.Imaging.ImageFormat]::Png); \
        [Convert]::ToBase64String($ms.ToArray()) }";
    let out = capture("powershell", &["-NoProfile", "-STA", "-Command", script])?;
    let encoded = String::from_utf8_lossy(&out);
    let encoded = encoded.trim();
    if encoded.is_empty() {
        return Ok(None);
    }
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map(Some)
        .map_err(|e| io::Error::other(format!("clipboard image is not base64: {e}")))
}

#[cfg(windows)]
fn read_text() -> io::Result<Option<String>> {
    first_text(&[(
        "powershell",
        &["-NoProfile", "-Command", "Get-Clipboard -Raw"],
    )])
}

/// Text from the first of `tools` that answers.
fn first_text(tools: &[(&str, &[&str])]) -> io::Result<Option<String>> {
    let mut last_error = None;
    for (program, args) in tools {
        match capture(program, args) {
            Ok(out) => return Ok(Some(String::from_utf8_lossy(&out).into_owned())),
            Err(e) => last_error = Some(e),
        }
    }
    match last_error {
        Some(e) if e.kind() == io::ErrorKind::NotFound => Err(e),
        _ => Ok(None),
    }
}

/// The first of [`IMAGE_TYPES`] listed in `types`, one MIME type per line.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn preferred_image_type(types: &str) -> Option<&'static str> {
    IMAGE_TYPES
        .iter()
        .find(|wanted| types.lines().any(|line| line.trim() == **wanted))
        .copied()
}

/// Bytes of an AppleScript `«data XXXXhex»` literal.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn decode_applescript_data(out: &str) -> Option<Vec<u8>> {
    let body = out.trim().strip_prefix("«data ")?.strip_suffix('»')?;
    let hex = body.get(4..)?;
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Run `program` and return what it printed, giving up after
/// [`READ_TIMEOUT`].
fn capture(program: &str, args: &[&str]) -> io::Result<Vec<u8>> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return Err(io::Error::other(format!("{program} has no output")));
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let result = stdout
            .take(MAX_READ_BYTES)
            .read_to_end(&mut out)
            .map(|_| out);
        let _ = tx.send(result);
    });
    let Ok(read) = rx.recv_timeout(READ_TIMEOUT) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "{program} did not answer within {}s",
                READ_TIMEOUT.as_secs()
            ),
        ));
    };
    let out = read?;
    // A tool still writing past the cap would block on a full pipe forever.
    if out.len() as u64 >= MAX_READ_BYTES {
        let _ = child.kill();
    }
    let status = child.wait()?;
    if !status.success() && (out.len() as u64) < MAX_READ_BYTES {
        return Err(io::Error::other(format!("{program} exited with {status}")));
    }
    Ok(out)
}

/// Put `text` on the clipboard.
///
/// Writes an OSC 52 escape to `out`, which most terminals (and SSH
/// sessions) honour, and also pipes the text to the platform clipboard
/// tool, because VTE-based terminals such as GNOME Console ignore OSC 52.
///
/// # Errors
/// Returns the error from writing the escape to `out`.
pub fn copy(out: &mut impl Write, text: &str) -> io::Result<()> {
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

    #[test]
    fn picks_the_preferred_listed_image_type() {
        let listed = "text/plain\nimage/jpeg\nimage/png\n";
        assert_eq!(preferred_image_type(listed), Some("image/png"));
        assert_eq!(preferred_image_type("text/plain\nUTF8_STRING"), None);
    }

    #[test]
    fn decodes_applescript_png_data() {
        assert_eq!(
            decode_applescript_data("«data PNGf89504E47»\n"),
            Some(vec![0x89, 0x50, 0x4E, 0x47])
        );
        assert_eq!(decode_applescript_data("missing value"), None);
    }

    #[cfg(unix)]
    #[test]
    fn capture_returns_output_and_reports_failure() {
        assert_eq!(capture("printf", &["abc"]).unwrap(), b"abc");
        assert!(capture("false", &[]).is_err());
        let missing = capture("aura-no-such-clipboard-tool", &[]).unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    }
}
