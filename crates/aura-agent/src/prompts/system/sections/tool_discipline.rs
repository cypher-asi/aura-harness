//! Phase 4b prompt discipline rules.
//!
//! Codifies the tool-call patterns the harness actively enforces at
//! runtime (Phase 1's 6000-byte `write_file` chunk guard, Phase 2a's
//! `ForceToolCallNextTurn` hint, Phase 4a's narration budget) so the
//! model has the same rules visible in-context and stops triggering
//! the guards in the first place.
//!
//! The literal body is exported as a constant so the assembled-prompt
//! snapshot tests can assert on a single golden string without
//! introducing a new snapshot dependency.

/// Golden text for the `Tool-call discipline` section. The production
/// prompt builders splice this in verbatim; the snapshot tests assert
/// that each bullet survives into the fully assembled prompt.
pub const TOOL_CALL_DISCIPLINE_SECTION: &str = "\
Tool-call discipline:
- write_file must stay under 6000 bytes per call. If the file will be larger, create only the module doc + imports + one stub in your first write_file, then use edit_file with append_after_eof for the rest.
- After any read_file or search_code call, your next turn must either call another tool or submit a tool_result-producing action. Do not emit a multi-paragraph plan between tool calls.
- Never issue two search_code calls whose patterns share an alternation term (e.g. \"foo|bar\" then \"bar|baz\"). Consolidate into one refined query first.
- If your last two turns produced no tool calls, the next turn MUST be a single tool call. Prefer read_file or write_file (skeleton) over more exploration.
";
