use super::*;
use crate::app::{DetailView, PanelFocus, SidePanelTab};
use crate::events::{TaskState, TodoState, TodoView, UiCommand, UiEvent};
use tokio::sync::mpsc;

use TaskState::{Completed as C, Failed as F, Running as R};
use TodoState::{Completed as Done, InProgress as Doing, Pending as Todo};

mod support;
use support::*;

mod plan_files;
mod plans;
mod requests;
mod rounds;
