use super::layout::{CardKind, Spot, WireKind, MAX_LINES};
use super::timeline::MarkKind;
use super::*;
use crate::app::file_view::FileBody;
use crate::app::{ChatTab, DetailView};
use crate::events::{AgentSummary, FlowBench, FlowKind, FlowUsage, TraceNodeStatus, UiCommand};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

const LEAD: &str = "aa";
const CODER: &str = "bb";
const TESTER: &str = "cc";

mod support;
use support::*;

mod bands;
mod cards;
mod folding;
mod long_flows;
mod navigation;
mod opening;
mod timeline;
mod zoom;
