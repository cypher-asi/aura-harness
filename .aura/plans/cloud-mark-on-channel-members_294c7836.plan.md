---
name: Cloud mark on channel members
overview: Show the same cloud mark the PROJECTS list uses after a member's name in a channel's MEMBERS tab when that member runs in the cloud. This is a renderer-only change in the terminal UI, plus a test and a changelog entry.
todos:
- id: conflicts
  content: Confirm merge conflicts in projects.rs and CHANGELOG.md are resolved so the crate builds
  status: completed
- id: badge
  content: Pass app.cloud_badge into the member name row's RowBadge in channel_side.rs
  status: completed
- id: split
  content: Extract member_icon helper to keep member_rows under 50 lines; update module doc diagram
  status: completed
- id: test
  content: Add channel_members.rs test for own and foreign cloud members and no mark on local ones
  status: completed
- id: changelog
  content: Add an Unreleased CHANGELOG entry
  status: completed
- id: verify
  content: Run fmt, clippy -D warnings, aura-ui-terminal tests and the file size check
  status: completed
---

# Cloud mark on channel members

Show the same cloud mark the PROJECTS list uses after a member's name in a channel's MEMBERS tab when that member runs in the cloud. This is a renderer-only change in the terminal UI, plus a test and a changelog entry.

## Todos

- [x] Confirm merge conflicts in projects.rs and CHANGELOG.md are resolved so the crate builds
- [x] Pass app.cloud_badge into the member name row's RowBadge in channel_side.rs
- [x] Extract member_icon helper to keep member_rows under 50 lines; update module doc diagram
- [x] Add channel_members.rs test for own and foreign cloud members and no mark on local ones
- [x] Add an Unreleased CHANGELOG entry
- [x] Run fmt, clippy -D warnings, aura-ui-terminal tests and the file size check

<!-- end of generated header: edit below -->

## How it works today

A channel's MEMBERS tab is drawn by `render_channel_members` and `member_rows` in `crates/ui/aura-ui-terminal/src/renderer/channel_side.rs`. Each member takes two rows. The first row has the `@` or spinner, the permanent agent's name and the code on the right. The second row says what the member is working on. The name row is built with `tree_row`, and its badge slot is always `None` today.

The PROJECTS list already marks cloud agents. In `renderer/agents/projects.rs`, `agent_line` calls `app.cloud_badge(&agent.id)` (defined in `app/host_tab/mod.rs`). That call returns ` ☁ ` for your own cloud agent, or ` ☁  alice` with a "foreign" flag for another user's. The result becomes a `RowBadge`, muted for your own agents and faint for someone else's. The data is already there for channel members: the cloud publisher in `crates/surface/aura-app/src/event_loop/cloud/publish.rs` sends `UiCommand::SetPlacement` for every placed agent, not only the agent on screen. So no backend or protocol change is needed.

## The change

In `channel_side.rs`, pass the cloud badge into the member's name row, exactly as PROJECTS does. The mark sits right after the name, for example `rust-engineer ☁`, with the code still on the right. `tree_row` shortens a long name to make room for the badge, and drops the badge on rows too narrow for it. Both behaviours come for free.

- Add a small helper, `member_badge(theme, cloud: Option<&(String, bool)>) -> Option<RowBadge>`. It colors the badge muted for your own agent and faint for another user's, matching `agent_badge` in `projects.rs`.
- In `member_rows`, compute `let cloud = app.cloud_badge(&member.id);` and pass `(member_name(member), member_badge(theme, cloud.as_ref()))` to `tree_row` in place of `None`.
- `member_rows` is close to the 50-line limit. Move the icon choice (spinner, `@`, or blank) into its own `member_icon(app, member, theme)` helper so the function stays under the limit.
- Update the module doc's diagram at the top of the file to show a cloud member, for example `@  rust-engineer ☁        REN3`.

Subagents get the mark too, because the badge depends only on the agent's own placement. Plan mode's `(P)` badge stays out of scope here.

## Tests and changelog

Add a test to `crates/ui/aura-ui-terminal/src/renderer/tests/channel_members.rs`. It builds on `members_app()`, sends `UiCommand::SetPlacement` with `PlacementView::Cloud` for `r1`, renders, and checks three things:

- The `REN3` row contains `rust-engineer ☁`.
- The coordinator row has no `☁`.
- Another user's cloud member (placement with `owner: Some("Alice")`) shows `☁  Alice`.

Add a short "Channel members show a cloud for cloud agents" entry under `[Unreleased]` in `CHANGELOG.md`, following the existing headings.

## Before starting

The working tree has unresolved merge conflicts in `crates/ui/aura-ui-terminal/src/renderer/agents/projects.rs` (around `agent_badge`) and in `CHANGELOG.md`. The crate will not compile until they are resolved. This change does not touch `projects.rs`, but those conflicts must be fixed first, or by whoever is committing the outstanding work, before tests can run.

Check with `cargo fmt --all`, `cargo clippy -p aura-ui-terminal --all-targets -- -D warnings`, `cargo test -p aura-ui-terminal channel_members`, and `bash scripts/check_file_size.sh`.
