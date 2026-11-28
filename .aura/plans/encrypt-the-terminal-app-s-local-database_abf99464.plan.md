---
name: Encrypt the terminal app's local database
overview: Have the terminal app, `aura -p` and aura-desktop open their RocksDB store sealed with aura-node's AES-256-GCM `SealCipher`. Existing plaintext stores get a one-time copy, verify and swap migration that keeps the original data until the sealed copy is proven. The shared migration code moves out of aura-node into aura-store-db so both use the same code.
todos:
- id: branch
  content: Stash pre-existing changes and recreate the branch from origin/main
  status: pending
- id: store-db-migration
  content: Move the migration state machine into aura-store-db seal_migration, with marker key_check, full verify, rollback and FailAt hook
  status: pending
- id: seal-key
  content: Move the hex key and 0600 key-file helpers into aura-store-db and point aura-node at them
  status: pending
- id: node-wrapper
  content: Turn aura-node state_migration and write_sealed_marker into thin wrappers and keep its tests passing
  status: pending
- id: keychain
  content: Add the public KeychainSecret wrapper over SecretSlot in aura-auth
  status: pending
- id: local-store
  content: 'Add aura-app local_store: key sources, lock, sealed open, fallback and clear errors'
  status: pending
- id: wire
  content: Switch the session and headless call sites to local_store::open; set AURA_STATE_KEY_FILE in headless_print tests
  status: pending
- id: tests
  content: Write store-db verify/marker tests and the aura-app scenario tests
  status: pending
- id: checks
  content: Run fmt, clippy, tests and the file-size check for the changed crates
  status: pending
- id: ship
  content: Push, open the PR, merge latest main, rerun the tests, merge with a merge commit, report
  status: pending
---

# Encrypt the terminal app's local database

Have the terminal app, `aura -p` and aura-desktop open their RocksDB store sealed with aura-node's AES-256-GCM `SealCipher`. Existing plaintext stores get a one-time copy, verify and swap migration that keeps the original data until the sealed copy is proven. The shared migration code moves out of aura-node into aura-store-db so both use the same code.

## Todos

- [ ] Stash pre-existing changes and recreate the branch from origin/main
- [ ] Move the migration state machine into aura-store-db seal_migration, with marker key_check, full verify, rollback and FailAt hook
- [ ] Move the hex key and 0600 key-file helpers into aura-store-db and point aura-node at them
- [ ] Turn aura-node state_migration and write_sealed_marker into thin wrappers and keep its tests passing
- [ ] Add the public KeychainSecret wrapper over SecretSlot in aura-auth
- [ ] Add aura-app local_store: key sources, lock, sealed open, fallback and clear errors
- [ ] Switch the session and headless call sites to local_store::open; set AURA_STATE_KEY_FILE in headless_print tests
- [ ] Write store-db verify/marker tests and the aura-app scenario tests
- [ ] Run fmt, clippy, tests and the file-size check for the changed crates
- [ ] Push, open the PR, merge latest main, rerun the tests, merge with a merge commit, report

<!-- end of generated header: edit below -->

## How it works today

The app opens its store in `open_workspace` (`crates/surface/aura-app/src/session/mod.rs`) and `open_stores` (`crates/surface/aura-app/src/headless/mod.rs`). Both go through `session_bootstrap::open_store`, which calls `RocksStore::open(path, false)`, so nothing is sealed. aura-desktop goes through `aura_app::session::start`, so it is covered by the same change.

aura-node already has all the parts:
- **Value sealing.** `aura_store_db::SealCipher` (AES-256-GCM, `AURASEAL` envelope). `RocksStore::open_sealed` plus every `for_store(&store)` sub-store (AgentStore, ChannelStore, TraceStore) seal values transparently. Keys and pure counters stay plaintext.
- **Copy step.** `aura_store_db::seal_db_copy` copies a plaintext DB into a fresh sealed DB.
- **Orchestration.** `aura-node/src/state_migration.rs` runs temp dir, rename swap, `.aura-sealed` marker and backup cleanup, with crash recovery.
- **Key.** It comes from the CoCo CDH/KBS in production. In dev it comes from `LocalKeyFileDekProvider`: a 64-hex-char file at `AURA_STATE_KEY_FILE`, created `0600`.

The gaps for the app:
- aura-app cannot use the orchestration without depending on aura-node.
- The copy is only checked by "it opens". There is no count or per-key read-back.
- A wrong key is only noticed when a read fails, and the app swallows some read errors (for example `conversation_history(..).ok()`). The user would see an empty chat instead of an error.
- Nothing stops two `aura` processes from migrating the same data dir at once.

## Where the key lives

I chose **the OS keychain, falling back to an owner-only key file in the data dir**, following your rule. The code already supports the keychain: `aura-auth`'s `SecretSlot` stores the login tokens in the keychain (Keychain / DPAPI / Secret Service). When no keychain backend exists it writes an atomic `0600` file instead. I reuse it as is, and no passphrase is involved.

- **Keychain entry.** Service `aura`, user `store-key:<store-id>`. The value is the same 64-hex encoding aura-node's key file uses.
- **Store id.** A random UUID in `<data_dir>/.aura-store-id` (not secret). Each data dir gets its own key, and moving the directory doesn't orphan it. The default data dir is `./aura_data`, per launch folder, so a user may have several.
- **Fallback file.** `<data_dir>/.aura-store-key`, mode `0600`, used only when the keychain is unavailable (for example headless Linux). It is logged at WARN by `SecretSlot`.
- **Override.** `AURA_STATE_KEY_FILE`, the same variable aura-node uses, switches to file-only. Tests, CI and server boxes use it, so tests never touch a developer's keychain. `tests/headless_print.rs` sets it.

The trade-off goes in the report. Losing the keychain entry (keychain reset, data dir copied to another machine) makes the store unreadable, with a clear error. Users who want a portable data dir can set `AURA_STATE_KEY_FILE`.

## Step 1: Move the migration into aura-store-db and make it stricter

This adds a module directory `crates/store/aura-store-db/src/seal_migration/` (each file under 500 lines). It holds aura-node's state machine, moved, plus these additions:

- `marker.rs` reads and writes the `.aura-sealed` marker. Writes are atomic: tmp file, fsync, rename. The marker gains an optional `key_check`: the hex of `cipher.seal(b"aura-store-key-check")`. `check_key` opens it to reject a wrong key before the store is used. Markers written before this change have no check and skip it, so aura-node's existing pods keep booting.
- `verify.rs` adds `verify_sealed_copy(src, dst, cipher)`. It walks every column family of both DBs in lockstep. Keys must match one to one, so the counts are equal, and every value is read back. Sealed column families must decrypt to the source bytes; plaintext column families must match byte for byte. Values the source already had sealed must open with this key; if not, it returns a `ForeignSealedValue` error. Extra column families in the destination must be empty.
- `mod.rs` holds `migrate_if_needed(data_dir, db_path, key_id, cipher)`. It runs copy, then full verify, then rename the plaintext to `<db>.plaintext-backup` and the copy to `<db>`, then write the marker, then verify the live DB opens and the key check passes. Only then does it delete the backup. If any step before the delete fails, it rolls back right away (restore the backup, drop the marker and temp dir) and returns an error, so the original is back at `<db>`. A failed backup delete is a warning; the next start finishes it. Crash recovery on the next start stays as it is in aura-node today.
- `SealMigrationError` (thiserror) carries paths and step names. `is_fatal()` is true only for a wrong or foreign key and for a failed rollback.
- A `test-support`-gated fault hook (`FailAt::{AfterCopy, AfterBackupRename, AfterSwap, AfterMarker}`) lets tests interrupt each step deterministically.
- A `seal_key` helper holds the hex key encode/decode and the `0600` create-new key file write, moved from aura-node's `providers.rs`.

`aura-node/src/state_migration.rs` becomes a thin wrapper that keeps its public names (`migrate_state_if_needed`, `migrating_dir`, `backup_dir`, the suffixes, `MigrationOutcome`). `sealing::write_sealed_marker` delegates to the shared marker and now gets the cipher, so it can write `key_check`. aura-node's behaviour is otherwise unchanged: sealed mode stays opt-in from env, and a failure there still refuses to boot. The bulk of the migration tests move with the code; aura-node keeps a wrapper smoke test.

## Step 2: Open the app's store sealed

A new `crates/surface/aura-app/src/local_store/` module (`mod.rs`, `keys.rs`, `tests/`) adds `local_store::open(data_dir, db_path) -> anyhow::Result<Arc<RocksStore>>`. It runs everything in `tokio::task::spawn_blocking`, because the keychain call, the migration and RocksDB are all blocking. The sequence:

1. Take an OS file lock on `<data_dir>/.aura-seal.lock` with `aura_exec_merge_queue::lock::FileLock`, so two processes can't migrate the same dir together. It waits up to a bounded time; past that it errors with "another aura process is preparing this data directory".
2. **Marker present (already sealed).** Load the key. A missing key gives `MissingKey` and a wrong key gives `WrongKey`; both are hard errors naming the DB and where the key was looked for. Never generate a new key here. Clean up leftovers, then `open_sealed`.
3. **No marker.** Recover any interrupted migration. Then load the key, or generate, save and read it back to confirm it persisted. If the store is empty, write the marker and open sealed (fresh install). If it has data, migrate.
4. **Migration or key setup fails with a non-fatal error.** Log a WARN ("keeping the unencrypted database for now; will retry next start") and open the original plaintext store with `RocksStore::open`. Fatal errors stop startup with the message.

The two call sites change by one line each. `session/mod.rs` (449 lines) and `headless/mod.rs` (447 lines) stay under the 500-line limit. aura-app gains a normal dependency on `zeroize`, a dev-dependency feature `aura-store-db/test-support`, and a public `KeychainSecret` wrapper over `SecretSlot` in `aura-auth`. No new third-party crates.

## What happens to the old plaintext data

After the sealed copy passes the full verify and the swapped-in DB opens with the key check, `<db>.plaintext-backup` is **deleted**. Keeping it would leave the plaintext on disk, which defeats the change. The INFO log records the counts and the path removed. Until that point the original is always intact: either at `<db>`, or in the backup dir, which recovery restores. The PR notes that `remove_dir_all` does not shred disk blocks; full-disk encryption covers that.

Other plaintext files in the data dir (chat notes, approvals, process history, the eval recorder's scratch store opened by `session_bootstrap::open_store`) are outside the store. The PR lists them as a follow-up. RUS203 has been told to keep owed reports in the store via `for_store`, or to add any new column family to all three lists in `open.rs` and `migrate.rs`.

## Tests

These go in `aura-app/src/local_store/tests/`, using the key-file source:
- A fresh data dir opens sealed: the marker exists, the key file is `0600`, data round-trips, and the raw files don't contain a known string.
- A plaintext store with channel posts, post summaries, agent registry rows and record entries (prompts) migrates. Everything reads back through the typed APIs, the backup is gone, and no file under the data dir contains the known plaintext string. The test first asserts the string was on disk before migrating.
- At each `FailAt` step the open still succeeds on the original plaintext data with everything readable and no marker. The next open migrates. Crash states rebuilt on disk (backup only; sealed copy live without a marker; partial temp dir) recover without loss.
- A missing key or a wrong key with a marker gives `MissingKey` / `WrongKey`, with readable messages, and the DB files stay unchanged.

`aura-store-db` gets unit tests for `verify_sealed_copy` (a tampered value, a missing key, an extra row, a foreign-sealed value) and for the marker key check.

## Shipping

The workspace has 62 modified and 8 untracked files from before this task, none of mine, and the branch is 84 commits behind `origin/main`. I'll keep them with `git stash push -u -m "pre-existing changes, not RUS205"` and recreate the branch from `origin/main`. Nothing gets committed on anyone's behalf. Then:
- Run `cargo fmt`, `cargo clippy -D warnings` and `cargo test` for aura-store-db, aura-auth, aura-app and aura-node, plus `bash scripts/check_file_size.sh`.
- Push without force, open a PR with the note (key location, migration, backup handling, first-start behaviour) and leave `CHANGELOG.md` alone.
- Merge the latest main, rerun the tests and merge with `gh pr merge --merge`.
