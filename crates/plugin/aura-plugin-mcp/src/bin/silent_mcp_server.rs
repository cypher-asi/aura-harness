//! Test fixture MCP server that accepts one request and never answers.

use std::io::{self, BufRead};
use std::time::Duration;

fn main() {
    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);
    std::thread::sleep(Duration::from_secs(10));
}
