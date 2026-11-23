//! How amounts, durations, token counts and headlines read.

use super::super::format::{duration, headline, money, tokens};

#[test]
fn numbers_read_naturally() {
    assert_eq!(duration(850), "850ms");
    assert_eq!(duration(42_400), "42s");
    assert_eq!(duration(185_000), "3m 05s");
    assert_eq!(tokens(950), "950");
    assert_eq!(tokens(126_000), "126k");
    assert_eq!(tokens(1_400_000), "1.4M");
    assert_eq!(money(0.0), "free");
    assert_eq!(money(0.004), "$0.0040");
    assert_eq!(headline("  \nfirst line\nsecond", 80), "first line");
    assert_eq!(headline("abcdef", 4), "abc…");
}
