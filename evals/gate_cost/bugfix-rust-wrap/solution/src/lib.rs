/// Greedy word wrap. Words are separated by whitespace; each line holds
/// as many words as fit in `width` columns, joined by single spaces. A
/// word longer than `width` gets a line of its own. Empty input gives
/// no lines.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
        } else if line.len() + 1 + word.len() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::take(&mut line));
            line.push_str(word);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}
#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn fills_lines_exactly() {
        assert_eq!(wrap("aaa bbb ccc", 7), vec!["aaa bbb", "ccc"]);
    }

    #[test]
    fn keeps_long_words() {
        assert_eq!(wrap("abcdefgh ij", 4), vec!["abcdefgh", "ij"]);
    }
}
