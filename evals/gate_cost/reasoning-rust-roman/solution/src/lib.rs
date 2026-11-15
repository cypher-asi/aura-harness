const NUMERALS: [(u32, &str); 13] = [
    (1000, "M"),
    (900, "CM"),
    (500, "D"),
    (400, "CD"),
    (100, "C"),
    (90, "XC"),
    (50, "L"),
    (40, "XL"),
    (10, "X"),
    (9, "IX"),
    (5, "V"),
    (4, "IV"),
    (1, "I"),
];

/// Roman numeral for `n`, or `None` outside 1..=3999.
pub fn to_roman(mut n: u32) -> Option<String> {
    if !(1..=3999).contains(&n) {
        return None;
    }
    let mut out = String::new();
    for (value, numeral) in NUMERALS {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    Some(out)
}

/// Value of a Roman numeral in canonical form (the form `to_roman`
/// produces). Anything else, including "IIII", "VX", "IC" and "",
/// gives `None`.
pub fn from_roman(s: &str) -> Option<u32> {
    let mut rest = s;
    let mut total = 0;
    for (value, numeral) in NUMERALS {
        while let Some(tail) = rest.strip_prefix(numeral) {
            total += value;
            rest = tail;
        }
    }
    if !rest.is_empty() || to_roman(total).as_deref() != Some(s) {
        return None;
    }
    Some(total)
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
