//! Proving a sealed copy holds exactly its source's data.
//!
//! Both databases are walked column family by column family in key
//! order, side by side. Every key must appear in both, once, so the
//! counts match, and every value is read back: values of
//! [`SEALED_VALUE_CFS`] must decrypt to the source bytes, every other
//! value must match byte for byte.

use std::path::Path;

use rocksdb::{IteratorMode, Options};

use super::SealMigrationError;
use crate::error::backend;
use crate::migrate::{Db, SEALED_VALUE_CFS};
use crate::seal::SealCipher;

/// Check the sealed copy at `dst` against the plaintext database at
/// `src`, both opened read-only. Returns the number of keys compared.
///
/// # Errors
/// [`SealMigrationError::Mismatch`] for any difference,
/// [`SealMigrationError::ForeignSealedValue`] for a source value sealed
/// under another key, and open or read failures.
pub fn verify_sealed_copy(
    src: &Path,
    dst: &Path,
    cipher: &SealCipher,
) -> Result<u64, SealMigrationError> {
    let opening = |e| SealMigrationError::store("opening the source to verify", src, backend(e));
    let names = Db::list_cf(&Options::default(), src).map_err(opening)?;
    let src_db =
        Db::open_cf_for_read_only(&Options::default(), src, &names, false).map_err(opening)?;
    verify_against(
        &Source {
            db: &src_db,
            path: src,
            cf_names: &names,
        },
        dst,
        cipher,
    )
}

/// The database a copy was made from, already open.
pub(crate) struct Source<'a> {
    pub(crate) db: &'a Db,
    pub(crate) path: &'a Path,
    pub(crate) cf_names: &'a [String],
}

/// [`verify_sealed_copy`] against a source the caller holds open.
pub(crate) fn verify_against(
    src: &Source<'_>,
    dst: &Path,
    cipher: &SealCipher,
) -> Result<u64, SealMigrationError> {
    let opening =
        |e| SealMigrationError::store("opening the sealed copy to verify", dst, backend(e));
    let dst_names = Db::list_cf(&Options::default(), dst).map_err(opening)?;
    let dst_db =
        Db::open_cf_for_read_only(&Options::default(), dst, &dst_names, false).map_err(opening)?;
    let copy = Copy {
        db: &dst_db,
        path: dst,
        cipher,
    };

    let mut keys = 0u64;
    for name in src.cf_names {
        keys += compare_family(src, &copy, name)?;
    }
    for name in dst_names.iter().filter(|n| !src.cf_names.contains(n)) {
        copy.expect_empty(name)?;
    }
    Ok(keys)
}

/// The sealed copy being checked.
struct Copy<'a> {
    db: &'a Db,
    path: &'a Path,
    cipher: &'a SealCipher,
}

impl Copy<'_> {
    fn read_error(&self, e: rocksdb::Error) -> SealMigrationError {
        SealMigrationError::store("reading the sealed copy to verify", self.path, backend(e))
    }

    fn mismatch(&self, detail: String) -> SealMigrationError {
        SealMigrationError::mismatch(self.path, detail)
    }

    /// A family only the copy has (the source predates it) must be empty.
    fn expect_empty(&self, name: &str) -> Result<(), SealMigrationError> {
        let family = self
            .db
            .cf_handle(name)
            .ok_or_else(|| self.mismatch(format!("column family `{name}` vanished")))?;
        match self.db.iterator_cf(&family, IteratorMode::Start).next() {
            None => Ok(()),
            Some(Err(e)) => Err(self.read_error(e)),
            Some(Ok((key, _))) => Err(self.mismatch(format!(
                "column family `{name}` has key {} the source lacks",
                key_label(&key)
            ))),
        }
    }
}

/// Walk family `name` of both databases side by side; returns its keys.
fn compare_family(
    src: &Source<'_>,
    copy: &Copy<'_>,
    name: &str,
) -> Result<u64, SealMigrationError> {
    let src_family = src
        .db
        .cf_handle(name)
        .ok_or_else(|| copy.mismatch(format!("source column family `{name}` vanished")))?;
    let dst_family = copy
        .db
        .cf_handle(name)
        .ok_or_else(|| copy.mismatch(format!("column family `{name}` is missing")))?;
    let sealed = SEALED_VALUE_CFS.contains(&name);
    let src_read =
        |e| SealMigrationError::store("reading the source to verify", src.path, backend(e));

    let mut theirs = src.db.iterator_cf(&src_family, IteratorMode::Start);
    let mut ours = copy.db.iterator_cf(&dst_family, IteratorMode::Start);
    let mut keys = 0u64;
    loop {
        match (theirs.next(), ours.next()) {
            (None, None) => return Ok(keys),
            (Some(row), None) => {
                let (key, _) = row.map_err(src_read)?;
                return Err(copy.mismatch(format!("`{name}` lacks key {}", key_label(&key))));
            }
            (None, Some(row)) => {
                let (key, _) = row.map_err(|e| copy.read_error(e))?;
                return Err(copy.mismatch(format!("`{name}` has extra key {}", key_label(&key))));
            }
            (Some(theirs_row), Some(ours_row)) => {
                let (src_key, src_value) = theirs_row.map_err(src_read)?;
                let (dst_key, dst_value) = ours_row.map_err(|e| copy.read_error(e))?;
                if src_key != dst_key {
                    return Err(copy.mismatch(format!(
                        "`{name}` has key {} where the source has {}",
                        key_label(&dst_key),
                        key_label(&src_key)
                    )));
                }
                let row = Row {
                    family: name,
                    key: &src_key,
                    src: &src_value,
                    dst: &dst_value,
                };
                check_value(src.path, copy, &row, sealed)?;
                keys += 1;
            }
        }
    }
}

/// One key's value in both databases.
struct Row<'a> {
    family: &'a str,
    key: &'a [u8],
    src: &'a [u8],
    dst: &'a [u8],
}

fn check_value(
    src_path: &Path,
    copy: &Copy<'_>,
    row: &Row<'_>,
    sealed_family: bool,
) -> Result<(), SealMigrationError> {
    let matches = if !sealed_family {
        row.dst == row.src
    } else if SealCipher::is_sealed(row.src) {
        // Copied verbatim: it must already be sealed under this key.
        if copy.cipher.open(row.src).is_err() {
            return Err(SealMigrationError::ForeignSealedValue {
                path: src_path.to_path_buf(),
                column_family: row.family.to_string(),
            });
        }
        row.dst == row.src
    } else {
        SealCipher::is_sealed(row.dst)
            && copy
                .cipher
                .open(row.dst)
                .is_ok_and(|plain| plain == row.src)
    };
    if matches {
        Ok(())
    } else {
        Err(copy.mismatch(format!(
            "`{}` value under key {} does not read back as the source's",
            row.family,
            key_label(row.key)
        )))
    }
}

/// A key for an error message: printable keys as text, others as hex,
/// cut to a readable length.
fn key_label(key: &[u8]) -> String {
    const MAX: usize = 48;
    let shown = &key[..key.len().min(MAX)];
    let more = if key.len() > MAX { "…" } else { "" };
    if shown.iter().all(|b| b.is_ascii_graphic()) {
        format!("`{}{more}`", String::from_utf8_lossy(shown))
    } else {
        let hex: String = shown.iter().map(|b| format!("{b:02x}")).collect();
        format!("0x{hex}{more}")
    }
}
