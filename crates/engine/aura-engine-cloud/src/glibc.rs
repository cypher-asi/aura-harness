//! Which glibc a Linux aura binary needs, and which the machines have.
//!
//! A dynamically linked binary lists the glibc symbol versions it needs
//! (`GLIBC_2.34`, `GLIBC_2.2.5`, ...) as NUL-terminated strings in its
//! dynamic string table, referenced from `.gnu.version_r`. The newest of
//! them is the oldest glibc the binary loads on; on an older one the
//! loader refuses it with "version `GLIBC_2.N' not found".
//!
//! ## Assumptions
//!
//! - The scan matches whole NUL-delimited strings only, so stray bytes
//!   that happen to spell `GLIBC_2.` inside other data (including this
//!   module's own pattern in a release build) don't count.
//! - A statically linked or musl binary has no such strings and needs
//!   nothing.

use std::fmt;

/// glibc of the boxd machine image (Ubuntu 24.04). A binary that needs a
/// newer one does not start there.
pub const MACHINE_GLIBC: GlibcVersion = GlibcVersion::new(2, 39);

/// The prefix of every glibc 2.x symbol version string.
const GLIBC_2_PREFIX: &[u8] = b"GLIBC_2.";

/// Longest version suffix looked at after the prefix (`2.2.5` style
/// names are a few bytes; anything longer is not a version).
const MAX_SUFFIX_LEN: usize = 16;

/// A glibc release, compared major first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlibcVersion {
    /// Major version (always 2 in practice).
    pub major: u32,
    /// Minor version: the 43 of 2.43, the 2 of 2.2.5.
    pub minor: u32,
}

impl GlibcVersion {
    /// `major.minor`.
    #[must_use]
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }
}

impl fmt::Display for GlibcVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The newest glibc version `binary` references, or `None` when it
/// references none (static, musl, or not an ELF file at all).
#[must_use]
pub fn required_glibc(binary: &[u8]) -> Option<GlibcVersion> {
    let mut newest = None;
    let mut from = 0;
    while let Some(offset) = binary
        .get(from..)
        .and_then(|rest| rest.iter().position(|byte| *byte == GLIBC_2_PREFIX[0]))
    {
        let at = from + offset;
        from = at + 1;
        if let Some(minor) = version_string_at(binary, at) {
            newest = newest.max(Some(GlibcVersion::new(2, minor)));
        }
    }
    newest
}

/// The minor version of a whole `GLIBC_2.<minor>[.<patch>...]\0` string
/// starting at `at`, if one starts there.
fn version_string_at(binary: &[u8], at: usize) -> Option<u32> {
    let starts_a_string = match at.checked_sub(1) {
        None => true,
        Some(before) => binary.get(before) == Some(&0),
    };
    if !starts_a_string {
        return None;
    }
    let suffix = binary.get(at..)?.strip_prefix(GLIBC_2_PREFIX)?;
    let window = suffix.get(..suffix.len().min(MAX_SUFFIX_LEN + 1))?;
    let end = window.iter().position(|byte| *byte == 0)?;
    let version = window.get(..end)?;
    let mut parts = version.split(|byte| *byte == b'.');
    let minor = parse_number(parts.next()?)?;
    parts
        .all(|part| parse_number(part).is_some())
        .then_some(minor)
}

fn parse_number(digits: &[u8]) -> Option<u32> {
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake dynamic string table: each name NUL-terminated, padded
    /// with unrelated bytes on both sides.
    fn strtab(names: &[&str]) -> Vec<u8> {
        let mut bytes = b"\x7fELF junk\0libc.so.6\0".to_vec();
        for name in names {
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(0);
        }
        bytes.extend_from_slice(b"more junk");
        bytes
    }

    #[test]
    fn the_newest_referenced_version_wins() {
        let cases: &[(&[&str], Option<u32>)] = &[
            (&["GLIBC_2.17", "GLIBC_2.34", "GLIBC_2.43"], Some(43)),
            (&["GLIBC_2.43", "GLIBC_2.17"], Some(43)),
            (&["GLIBC_2.2.5"], Some(2)),
            (&["GLIBC_2.2.5", "GLIBC_2.3.4", "GLIBC_2.14"], Some(14)),
            (&["GLIBC_PRIVATE", "GLIBC_ABI_DT_RELR"], None),
            (&[], None),
        ];
        for (names, minor) in cases {
            assert_eq!(
                required_glibc(&strtab(names)),
                minor.map(|minor| GlibcVersion::new(2, minor)),
                "{names:?}"
            );
        }
    }

    #[test]
    fn only_whole_strings_count() {
        let cases: &[&[u8]] = &[
            // Not preceded by a NUL.
            b"\0xGLIBC_2.99\0",
            // Not terminated.
            b"\0GLIBC_2.99",
            // Not a number.
            b"\0GLIBC_2.x\0",
            b"\0GLIBC_2.\0",
            b"\0GLIBC_2.9a\0",
            b"\0GLIBC_2.9..1\0",
            // Too long to be a version.
            b"\0GLIBC_2.11111111111111111111\0",
        ];
        for bytes in cases {
            assert_eq!(
                required_glibc(bytes),
                None,
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn a_version_at_the_very_start_counts() {
        assert_eq!(
            required_glibc(b"GLIBC_2.28\0"),
            Some(GlibcVersion::new(2, 28))
        );
    }

    #[test]
    fn versions_compare_and_print_as_releases() {
        assert!(GlibcVersion::new(2, 43) > MACHINE_GLIBC);
        assert!(GlibcVersion::new(2, 39) <= MACHINE_GLIBC);
        assert!(GlibcVersion::new(2, 4) < MACHINE_GLIBC);
        assert_eq!(MACHINE_GLIBC.to_string(), "2.39");
    }
}
