use super::*;
use crate::events::UiEvent;
use crossterm::event::KeyModifiers;

mod support;
use support::*;

mod classifier;
mod difficulty;
mod from_row;
mod opus;
mod suites;
