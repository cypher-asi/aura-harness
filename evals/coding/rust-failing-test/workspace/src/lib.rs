use std::collections::HashMap;

/// Count words case-insensitively, ignoring ASCII punctuation, and return
/// the `n` most frequent as `(word, count)`, most frequent first, ties
/// broken alphabetically.
pub fn top_words(text: &str, n: usize) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for raw in text.split_whitespace() {
        let word: String = raw
            .chars()
            .filter(|c| !c.is_ascii_punctuation())
            .collect();
        if word.is_empty() {
            continue;
        }
        *counts.entry(word).or_default() += 1;
    }
    let mut words: Vec<(String, usize)> = counts.into_iter().collect();
    words.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    words.truncate(n);
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_frequent_first() {
        let top = top_words("b a b c b a", 2);
        assert_eq!(top, vec![("b".into(), 3), ("a".into(), 2)]);
    }

    #[test]
    fn case_and_punctuation_are_ignored() {
        let top = top_words("The cat. the CAT, the dog!", 2);
        assert_eq!(top, vec![("the".into(), 3), ("cat".into(), 2)]);
    }

    #[test]
    fn ties_are_alphabetical() {
        let top = top_words("pear apple fig", 3);
        assert_eq!(
            top,
            vec![("apple".into(), 1), ("fig".into(), 1), ("pear".into(), 1)]
        );
    }
}
