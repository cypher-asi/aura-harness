//! Folder browser shown when creating a project.
//!
//! Lists the subfolders of one directory at a time. The user walks into
//! folders and back up, picks the folder to use, or creates a new one.

use std::io;
use std::path::{Path, PathBuf};

/// Folders listed per directory before the rest collapse into a "more" row.
const MAX_FOLDERS: usize = 500;

/// One row of the folder list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderRow {
    /// Pick the directory being browsed.
    UseThis,
    /// Go up to the parent directory.
    Parent,
    /// A subfolder, by name.
    Folder(String),
    /// Placeholder for folders hidden by the per-directory cap.
    More(usize),
}

/// Folder browser state.
#[derive(Debug)]
pub struct FolderPicker {
    dir: PathBuf,
    rows: Vec<FolderRow>,
    selected: usize,
    scroll: usize,
    show_hidden: bool,
    new_folder: Option<String>,
}

impl FolderPicker {
    /// Browse `dir`.
    ///
    /// # Errors
    /// Returns an error if `dir` cannot be read.
    pub fn open(dir: PathBuf) -> io::Result<Self> {
        let mut picker = Self {
            dir: PathBuf::new(),
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            show_hidden: false,
            new_folder: None,
        };
        picker.enter(dir, None)?;
        Ok(picker)
    }

    /// Directory being browsed.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Visible rows, in display order.
    #[must_use]
    pub fn rows(&self) -> &[FolderRow] {
        &self.rows
    }

    /// Index of the highlighted row.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Whether folders starting with `.` are listed.
    #[must_use]
    pub const fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// Name typed so far for a new folder, while one is being named.
    #[must_use]
    pub fn new_folder(&self) -> Option<&str> {
        self.new_folder.as_deref()
    }

    /// Keep the highlight inside a list `visible` rows tall, returning the
    /// first row to show.
    pub fn sync_scroll(&mut self, visible: usize) -> usize {
        self.scroll = super::follow_selection(self.scroll, self.selected, visible, self.rows.len());
        self.scroll
    }

    /// Move the highlight `delta` rows, clamped to the list.
    pub fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    /// Highlight the first row.
    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    /// Highlight the last row.
    pub fn select_last(&mut self) {
        self.selected = self.rows.len().saturating_sub(1);
    }

    /// Act on the highlighted row: `Use this folder` returns the directory
    /// being browsed, `..` goes up, and a folder is opened.
    ///
    /// # Errors
    /// Returns an error if the folder to open cannot be read.
    pub fn activate(&mut self) -> io::Result<Option<PathBuf>> {
        match self.rows.get(self.selected).cloned() {
            Some(FolderRow::UseThis) => Ok(Some(self.dir.clone())),
            Some(FolderRow::Parent) => self.go_up().map(|()| None),
            Some(FolderRow::Folder(name)) => {
                let dir = self.dir.join(name);
                self.enter(dir, None).map(|()| None)
            }
            Some(FolderRow::More(_)) | None => Ok(None),
        }
    }

    /// The highlighted folder without opening it, or the directory being
    /// browsed when `Use this folder` is highlighted.
    #[must_use]
    pub fn highlighted_folder(&self) -> Option<PathBuf> {
        match self.rows.get(self.selected)? {
            FolderRow::UseThis => Some(self.dir.clone()),
            FolderRow::Folder(name) => Some(self.dir.join(name)),
            FolderRow::Parent | FolderRow::More(_) => None,
        }
    }

    /// Go up to the parent directory, highlighting the folder just left.
    ///
    /// # Errors
    /// Returns an error if the parent directory cannot be read.
    pub fn go_up(&mut self) -> io::Result<()> {
        let Some(parent) = self.dir.parent().map(Path::to_path_buf) else {
            return Ok(());
        };
        let left = self
            .dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        self.enter(parent, left.as_deref())
    }

    /// Browse the home directory.
    ///
    /// # Errors
    /// Returns an error if the home directory is unknown or cannot be read.
    pub fn go_home(&mut self) -> io::Result<()> {
        let home = dirs::home_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory"))?;
        self.enter(home, None)
    }

    /// Show or hide folders starting with `.`.
    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        let highlighted = self.highlighted_name();
        let _ = self.enter(self.dir.clone(), highlighted.as_deref());
    }

    /// Start naming a new folder inside the directory being browsed.
    pub fn start_new_folder(&mut self) {
        self.new_folder = Some(String::new());
    }

    /// Stop naming a new folder without creating it.
    pub fn cancel_new_folder(&mut self) {
        self.new_folder = None;
    }

    /// Type `c` into the new folder's name.
    pub fn push_new_folder_char(&mut self, c: char) {
        if let Some(name) = self.new_folder.as_mut() {
            name.push(c);
        }
    }

    /// Delete the last character of the new folder's name.
    pub fn pop_new_folder_char(&mut self) {
        if let Some(name) = self.new_folder.as_mut() {
            name.pop();
        }
    }

    /// Create the folder being named and highlight it.
    ///
    /// # Errors
    /// Returns a message for the user if the name is invalid or the folder
    /// cannot be created.
    pub fn create_new_folder(&mut self) -> Result<(), String> {
        let name = self.new_folder.as_deref().unwrap_or_default().trim();
        if name.is_empty() {
            return Err("Folder name cannot be empty".to_string());
        }
        if name == "." || name == ".." || name.contains(['/', '\\']) {
            return Err(format!("Not a valid folder name: {name}"));
        }
        let name = name.to_string();
        let path = self.dir.join(&name);
        std::fs::create_dir(&path).map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => format!("{name} already exists"),
            _ => format!("Could not create {name}: {e}"),
        })?;
        self.new_folder = None;
        if name.starts_with('.') {
            self.show_hidden = true;
        }
        self.enter(self.dir.clone(), Some(&name))
            .map_err(|e| format!("Could not read {}: {e}", self.dir.display()))
    }

    fn highlighted_name(&self) -> Option<String> {
        match self.rows.get(self.selected)? {
            FolderRow::Folder(name) => Some(name.clone()),
            _ => None,
        }
    }

    /// Browse `dir`, highlighting the folder named `highlight` if listed.
    /// Leaves the picker unchanged when `dir` cannot be read.
    fn enter(&mut self, dir: PathBuf, highlight: Option<&str>) -> io::Result<()> {
        let mut folders: Vec<String> = std::fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| self.show_hidden || !name.starts_with('.'))
            .collect();
        folders.sort_by_key(|name| name.to_lowercase());

        let mut rows = vec![FolderRow::UseThis];
        if dir.parent().is_some() {
            rows.push(FolderRow::Parent);
        }
        let total = folders.len();
        rows.extend(folders.into_iter().take(MAX_FOLDERS).map(FolderRow::Folder));
        if total > MAX_FOLDERS {
            rows.push(FolderRow::More(total - MAX_FOLDERS));
        }

        self.selected = highlight
            .and_then(|name| {
                rows.iter()
                    .position(|row| matches!(row, FolderRow::Folder(found) if found == name))
            })
            .unwrap_or(0);
        self.dir = dir;
        self.rows = rows;
        self.scroll = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn names(picker: &FolderPicker) -> Vec<String> {
        picker
            .rows()
            .iter()
            .map(|row| match row {
                FolderRow::UseThis => "<use>".to_string(),
                FolderRow::Parent => "..".to_string(),
                FolderRow::Folder(name) => name.clone(),
                FolderRow::More(n) => format!("+{n}"),
            })
            .collect()
    }

    fn sample() -> (tempfile::TempDir, FolderPicker) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        fs::create_dir(dir.path().join("Docs")).unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("README.md"), "").unwrap();
        let picker = FolderPicker::open(dir.path().to_path_buf()).unwrap();
        (dir, picker)
    }

    #[test]
    fn lists_only_visible_folders_sorted() {
        let (_dir, picker) = sample();
        assert_eq!(names(&picker), ["<use>", "..", "Docs", "src"]);
    }

    #[test]
    fn hidden_folders_toggle() {
        let (_dir, mut picker) = sample();
        picker.toggle_hidden();
        assert_eq!(names(&picker), ["<use>", "..", ".git", "Docs", "src"]);
        picker.toggle_hidden();
        assert_eq!(names(&picker), ["<use>", "..", "Docs", "src"]);
    }

    #[test]
    fn open_folder_and_go_back_up() {
        let (dir, mut picker) = sample();
        picker.select_last();
        assert_eq!(picker.activate().unwrap(), None);
        assert_eq!(picker.dir(), dir.path().join("src"));
        assert_eq!(names(&picker), ["<use>", "..", "nested"]);

        picker.go_up().unwrap();
        assert_eq!(picker.dir(), dir.path());
        assert_eq!(
            picker.rows()[picker.selected()],
            FolderRow::Folder("src".into())
        );

        picker.select_last();
        picker.activate().unwrap();
        picker.move_by(1);
        assert_eq!(picker.rows()[picker.selected()], FolderRow::Parent);
        picker.activate().unwrap();
        assert_eq!(picker.dir(), dir.path());
    }

    #[test]
    fn use_this_folder_picks_the_current_directory() {
        let (dir, mut picker) = sample();
        assert_eq!(picker.activate().unwrap(), Some(dir.path().to_path_buf()));
        picker.select_last();
        assert_eq!(picker.highlighted_folder(), Some(dir.path().join("src")));
        picker.move_by(-2);
        assert_eq!(picker.highlighted_folder(), None);
    }

    #[test]
    fn creates_a_new_folder_and_highlights_it() {
        let (dir, mut picker) = sample();
        picker.start_new_folder();
        for c in "app".chars() {
            picker.push_new_folder_char(c);
        }
        picker.create_new_folder().unwrap();
        assert!(dir.path().join("app").is_dir());
        assert_eq!(picker.new_folder(), None);
        assert_eq!(
            picker.rows()[picker.selected()],
            FolderRow::Folder("app".into())
        );
    }

    #[test]
    fn rejects_bad_or_existing_folder_names() {
        let (_dir, mut picker) = sample();
        for name in ["", "  ", "a/b", "..", "src"] {
            picker.start_new_folder();
            for c in name.chars() {
                picker.push_new_folder_char(c);
            }
            assert!(picker.create_new_folder().is_err(), "{name:?}");
            assert!(picker.new_folder().is_some());
        }
    }

    #[test]
    fn unreadable_folder_leaves_the_picker_where_it_was() {
        let (dir, mut picker) = sample();
        picker.select_last();
        fs::remove_dir_all(dir.path().join("src")).unwrap();
        assert!(picker.activate().is_err());
        assert_eq!(picker.dir(), dir.path());
    }

    #[test]
    fn caps_large_directories() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..MAX_FOLDERS + 2 {
            fs::create_dir(dir.path().join(format!("d{i:04}"))).unwrap();
        }
        let picker = FolderPicker::open(dir.path().to_path_buf()).unwrap();
        assert_eq!(picker.rows().len(), MAX_FOLDERS + 3);
        assert_eq!(picker.rows().last(), Some(&FolderRow::More(2)));
    }
}
