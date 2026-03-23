//! HTTP API routes for tool management and service proxies.

mod tools;

pub use tools::{delete_tool_handler, get_tools_handler, install_tool_handler};
