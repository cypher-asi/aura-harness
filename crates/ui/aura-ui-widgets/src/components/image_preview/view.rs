//! [`ImageView`]: a pasted image open over the chat panel.

use super::ImagePreview;
use ratatui::text::Line;
use std::sync::Arc;

/// A pasted image open over the chat panel.
#[derive(Debug, Clone)]
pub struct ImageView {
    /// The image
    pub preview: Arc<ImagePreview>,
    /// `IMAGE 2`, for the panel header
    pub title: String,
    /// The image drawn at the size of the last frame
    drawn: Option<((u16, u16), Vec<Line<'static>>)>,
}

impl PartialEq for ImageView {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.preview, &other.preview) && self.title == other.title
    }
}

impl Eq for ImageView {}

impl ImageView {
    /// Show `preview` under `title`.
    #[must_use]
    pub const fn new(preview: Arc<ImagePreview>, title: String) -> Self {
        Self {
            preview,
            title,
            drawn: None,
        }
    }

    /// The image fitted to `cols` by `rows`, drawn again only when the
    /// size changes.
    pub fn lines(&mut self, cols: u16, rows: u16) -> &[Line<'static>] {
        if self
            .drawn
            .as_ref()
            .is_none_or(|(size, _)| *size != (cols, rows))
        {
            self.drawn = Some(((cols, rows), self.preview.lines(cols, rows)));
        }
        self.drawn
            .as_ref()
            .map_or(&[], |(_, lines)| lines.as_slice())
    }

    /// `1024×768 · PNG · 1.20 MiB`.
    #[must_use]
    pub fn info(&self) -> String {
        let kind = self
            .preview
            .media_type()
            .strip_prefix("image/")
            .unwrap_or("image")
            .to_uppercase();
        #[expect(clippy::cast_precision_loss, reason = "a file size to two places")]
        let mib = self.preview.bytes().len() as f64 / (1024.0 * 1024.0);
        match self.preview.size() {
            Some((w, h)) => format!("{w}×{h} · {kind} · {mib:.2} MiB"),
            None => format!("{kind} · {mib:.2} MiB"),
        }
    }

    /// Open the image in the system's viewer, from a copy in the temp
    /// folder.
    ///
    /// # Errors
    /// Returns why the copy could not be written or the viewer started.
    pub fn open_externally(&self) -> std::io::Result<std::path::PathBuf> {
        let extension = match self.preview.media_type() {
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "png",
        };
        let bytes = self.preview.bytes();
        let mut hasher = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(bytes, &mut hasher);
        let name = format!(
            "aura-image-{:016x}.{extension}",
            std::hash::Hasher::finish(&hasher)
        );
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, bytes)?;
        open_with_system(&path)?;
        Ok(path)
    }
}

/// Open `target`, a file or a web page, in the app the system picks for
/// it.
///
/// # Errors
/// Returns why the opener could not be started.
pub fn open_with_system(target: impl AsRef<std::ffi::OsStr>) -> std::io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    } else {
        std::process::Command::new("xdg-open")
    };
    let mut child = command
        .arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    // Reap the opener when it exits; it hands off to the viewer at once.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
