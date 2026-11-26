// Loop counters in these tests are deliberately small (<256); the `as u8`
// narrowings are safe by construction. TODO(W5): migrate to `u8::try_from`
// once the `cast_possible_truncation` lint is promoted workspace-wide.
#![allow(clippy::cast_possible_truncation)]

mod agent_state;
mod append;
mod inbox;
mod open;
mod record_scan;
mod sealed;
mod support;
