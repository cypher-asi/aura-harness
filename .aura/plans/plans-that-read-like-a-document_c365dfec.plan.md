---
name: Plans that read like a document
overview: Change the guidance the agent gets for writing plans so a plan reads as a short document in plain sentences, like the Cursor example, with code formatting kept for the things a reader would actually type or search for.
todos:
- id: prompt
  content: Rewrite the plan-writing bullet in plan_mode.rs for document-style plans
  status: completed
- id: tool-desc
  content: Update the create_plan `plan` parameter description in plan_tools.rs to match
  status: completed
- id: test
  content: Extend the plan_mode test to pin the document-style guidance
  status: completed
- id: changelog
  content: Add a short Unreleased entry to CHANGELOG.md
  status: completed
- id: gate
  content: Run fmt, clippy -D warnings and tests for aura-context-prompts and aura-exec-tools
  status: completed
---

# Plans that read like a document

Change the guidance the agent gets for writing plans so a plan reads as a short document in plain sentences, like the Cursor example, with code formatting kept for the things a reader would actually type or search for.

## Todos

- [x] Rewrite the plan-writing bullet in plan_mode.rs for document-style plans
- [x] Update the create_plan `plan` parameter description in plan_tools.rs to match
- [x] Extend the plan_mode test to pin the document-style guidance
- [x] Add a short Unreleased entry to CHANGELOG.md
- [x] Run fmt, clippy -D warnings and tests for aura-context-prompts and aura-exec-tools

<!-- end of generated header: edit below -->

## Why plans read badly today

The agent writes plans the way it is told to. Two pieces of text tell it: the plan-mode section of the system prompt in `crates/context/aura-context-prompts/src/system/sections/plan_mode.rs`, and the description of the `plan` parameter of the `create_plan` tool in `crates/exec/aura-exec-tools/src/plan_tools.rs`. Both ask for an "easy to scan" plan with generic `##` sections (Approach, Files to change, Risks), short bullet lists, a table for files, and `inline code` for paths and symbols.

The model follows that literally. A recent plan such as `merge-plans-into-the-tasks-side-tab` is mostly bullets made of identifiers, with nearly every noun in backticks and a file table that repeats the bullets above it. It is accurate, but it reads like a diff summary rather than something a person can follow from top to bottom.

The Cursor plan you shared works differently. Its headings say what each section is about ("What's already good (keep it)", "Where the time goes under load", "Phase 0: Measure first"). It opens with a paragraph of context in ordinary sentences. Its bullets are full sentences. Monospace shows up only for real paths, commands and type names, so those stand out instead of blending into a wall of code.

## What changes

Nothing in the renderer or the plan file format changes. This is a change to the words the model is given, in two places.

**The plan-mode prompt.** The "Make the plan easy to scan" bullet is replaced by guidance along these lines:

> Write the plan as a short document a person reads from top to bottom, not a checklist. The tool adds the title, overview and todo checklist, so start the body with `##` sections whose headings say what the section is about (for example "How it works today", "Phase 1: Measure first", "What could go wrong") rather than generic labels. Write plain sentences in short paragraphs, and open with a paragraph of context. Use a bullet list for steps or parallel items, each a full sentence. Use `inline code` only for what the reader would type or search for: file paths, commands, and names of functions, types and settings; write everything else as ordinary text. Bold a key phrase sparingly. Add a fenced code block only when the exact code matters, a table only to compare several items across the same columns, and a mermaid diagram when a flow is easier to see than to read. Keep it proportional to the task.

The bullet about todos keeps its rule that each todo is one short line.

**The `create_plan` tool description.** The `plan` parameter description gets the same advice in one or two sentences, so a model that only reads the tool schema (a subagent, or a session that is not in plan mode) writes plans the same way. The `create_plan` tool definition the coordinator sees is built from this text, so no other prompt needs editing.

## Tests

The existing test in `plan_mode.rs` checks that the section names `create_plan`, `/build` and `/plan`. It gains an assertion that the guidance asks for a readable document and limits inline code, so the wording cannot quietly drift back to the old checklist advice. The system prompt snapshots do not include this section, since it is appended at run time in `aura-app`, so they should not change; running the prompt crate's tests confirms that.

The gate is `cargo fmt --all -- --check`, `cargo clippy -p aura-context-prompts -p aura-exec-tools --all-targets --all-features -- -D warnings`, and `cargo test -p aura-context-prompts -p aura-exec-tools`.

## Open choice: rendering plans as documents

This plan changes how plans are written, not how they are shown. Today opening a plan from the TASKS tab loads the raw file in the file view (`open_plan_at` in `crates/ui/aura-ui-terminal/src/app/plans.rs`), so the frontmatter shows and everything is monospace, even in the desktop app, which already sets chat prose in Inter with inline code in monospace.

Showing a plan the way your screenshot does, as rendered Markdown with proportional body text, would mean a separate document view that reuses the chat's Markdown renderer and its prose rows. That is a much larger change, and it can be planned on its own if you want it.

## Risks

Longer prose could make plans longer. The guidance keeps "proportional to the task" and short paragraphs to hold that back. The model may also drop useful precision, such as exact file paths; the wording keeps code formatting for exactly those things, so they should survive.
