//! Splitting the arguments into words, keeping where each sits.

/// One word of the arguments: its text with quotes removed, and where it
/// sits in the raw arguments.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Word {
    pub(super) text: String,
    pub(super) start: usize,
    pub(super) end: usize,
}

impl Word {
    /// The word as typed in `args`, quotes included.
    pub(super) fn raw<'a>(&self, args: &'a str) -> &'a str {
        &args[self.start..self.end]
    }
}

/// Split `args` into words at whitespace; single or double quotes keep
/// whitespace inside a word.
pub(super) fn words(args: &str) -> Result<Vec<Word>, String> {
    let mut out = Vec::new();
    let mut current: Option<Word> = None;
    let mut quote: Option<char> = None;
    for (i, c) in args.char_indices() {
        if quote.is_none() && c.is_whitespace() {
            out.extend(current.take());
            continue;
        }
        let end = i + c.len_utf8();
        let word = current.get_or_insert_with(|| Word {
            text: String::new(),
            start: i,
            end,
        });
        word.end = end;
        match quote {
            Some(open) if c == open => quote = None,
            None if c == '"' || c == '\'' => quote = Some(c),
            _ => word.text.push(c),
        }
    }
    if quote.is_some() {
        return Err("Unclosed quote in /compare arguments".to_string());
    }
    out.extend(current);
    Ok(out)
}
