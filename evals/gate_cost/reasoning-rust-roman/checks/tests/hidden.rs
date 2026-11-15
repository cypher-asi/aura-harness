use roman::{from_roman, to_roman};

#[test]
fn round_trips_every_value() {
    for n in 1..=3999 {
        let s = to_roman(n).unwrap();
        assert_eq!(from_roman(&s), Some(n), "{s}");
    }
}

#[test]
fn rejects_out_of_range_and_non_canonical() {
    assert_eq!(to_roman(0), None);
    assert_eq!(to_roman(4000), None);
    for bad in ["", "IIII", "VX", "IC", "MMMM", "XM", "iv", "IIV", "VV"] {
        assert_eq!(from_roman(bad), None, "{bad}");
    }
}
