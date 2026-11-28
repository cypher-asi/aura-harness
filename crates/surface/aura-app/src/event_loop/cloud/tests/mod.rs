//! Tests of the event loop's cloud side: owners, forwarded turns,
//! machine rows and boot consoles.

use super::boot::boot_updates;
use super::forward::{forward, TitleSink, Titles, TurnEnd, TurnExtras, CLOSED_EARLY};
use super::machine::machine_rows;
use super::submit::runs_on_machine;
use super::*;

mod boot_tests;
mod forward_tests;
mod machine_tests;
mod owner_tests;
