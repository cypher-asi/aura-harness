use super::markdown::{parse_content_segments, ContentSegment};
use super::text::{display_width, wrap_words};
use crate::components::truncate_record_info;
use crate::events::CallUsage;
use crate::Theme;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
};

fn test_theme() -> Theme {
    Theme::cyber()
}

mod support;
use support::*;

mod agent_codes;
mod agent_commands;
mod agents_header;
mod bench_icon;
mod channel_history;
mod channel_info;
mod channel_member_chat;
mod channel_member_folds;
mod channel_members;
mod channel_queue;
mod channel_side_toggle;
mod channel_tasks;
mod channel_welcome;
mod channels;
mod chat_links;
mod chat_messages;
mod chat_view;
mod copy_buttons;
mod drag_scroll;
mod input_box;
mod input_overlays;
mod list_rows;
mod modals;
mod model_selector;
mod notifications;
mod panel_buttons;
mod placement_switch;
mod plan_badge;
mod record_info;
mod reply_footer;
mod roster;
mod side_panel;
mod side_panel_tabs;
mod sign_ins;
mod spawn_folding;
mod status_and_columns;
mod status_line;
mod terminal_inline;
mod terminal_pane;
mod text_layout;
mod todos;
mod turn_activity;
