/// Roman numeral for `n`, or `None` outside 1..=3999.
pub fn to_roman(n: u32) -> Option<String> {
    todo!()
}

/// Value of a Roman numeral in canonical form (the form `to_roman`
/// produces). Anything else, including "IIII", "VX", "IC" and "",
/// gives `None`.
pub fn from_roman(s: &str) -> Option<u32> {
    todo!()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_a_year() {
        assert_eq!(to_roman(1994).as_deref(), Some("MCMXCIV"));
        assert_eq!(from_roman("MCMXCIV"), Some(1994));
    }
}
