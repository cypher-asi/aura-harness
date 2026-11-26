//! Where the tabs' arrangement of projects is kept between runs.

use std::path::{Path, PathBuf};

/// The root agents of the projects in each tab, in tab order.
pub type Arrangement = Vec<Vec<String>>;

/// Where the arrangement is kept: beside the agent registry, in the data
/// directory the session resolves the same way from `dir`.
pub fn arrangement_file(dir: Option<&Path>) -> PathBuf {
    dir.map(Path::to_path_buf)
        .or_else(|| std::env::var("AURA_DATA_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("./aura_data"))
        .join("desktop_tabs")
}

/// The arrangement saved at `path`: a line per tab, of ids separated by
/// spaces. Empty when there is none to read.
pub fn load_arrangement(path: &Path) -> Arrangement {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .map(|line| line.split_whitespace().map(str::to_string).collect())
        .filter(|tab: &Vec<String>| !tab.is_empty())
        .collect()
}

/// Write `arrangement` to `path` for [`load_arrangement`].
///
/// # Errors
///
/// Fails when the file can't be written.
pub fn save_arrangement(path: &Path, arrangement: &Arrangement) -> std::io::Result<()> {
    let lines: Vec<String> = arrangement.iter().map(|tab| tab.join(" ")).collect();
    std::fs::write(path, lines.join("\n") + "\n")
}
