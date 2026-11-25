---
name: 'Channels: persistent #channels with a coordinator, spawned agents and subagents'
overview: 'Move the agent registry from agents.json into the shared RocksDB keyed by AgentId, then build Channels on the same store: a CHANNELS tab, a coordinator chat with @member routing and posts, and one tall right panel listing the channel''s projects, working agents and tasks.'
todos:
- id: fix-queued
  content: Don't show 'Queued' for a message routed to channel members
  status: completed
- id: fix-adopt
  content: Keep store and registry in step when registering the coordinator fails
  status: completed
- id: persist-digest
  content: Save each coordinator's last-summary time in the channel store
  status: completed
- id: side-menu
  content: Right-click menu in the right-hand channel panel
  status: completed
- id: perf
  content: Run agent_list_perf on a copy of real data, branch vs main
  status: in_progress
- id: merge-main
  content: Merge main into channels, resolve conflicts, run all checks
  status: in_progress
- id: merge
  content: Merge channels into main
  status: pending
---

# Channels: persistent #channels with a coordinator, spawned agents and subagents

Move the agent registry from agents.json into the shared RocksDB keyed by AgentId, then build Channels on the same store: a CHANNELS tab, a coordinator chat with @member routing and posts, and one tall right panel listing the channel's projects, working agents and tasks.

## Todos

- [x] Don't show 'Queued' for a message routed to channel members
- [x] Keep store and registry in step when registering the coordinator fails
- [x] Save each coordinator's last-summary time in the channel store
- [x] Right-click menu in the right-hand channel panel
- [~] Run agent_list_perf on a copy of real data, branch vs main
- [~] Merge main into channels, resolve conflicts, run all checks
- [ ] Merge channels into main

<!-- end of generated header: edit below -->

## Context

- Work in `.worktrees/channels` on branch `channels`. The worktree exists and is checked out at `67745a1f`. Do not touch `main`; it has uncommitted work.
- Follow `.cursor/rules.md`:
  - no warnings, clean fmt, and clippy with `-D warnings`
  - no `unwrap`/`expect` in production code
  - `thiserror` for library errors, `tracing` for logs
  - `tempfile` in tests
  - each new storage module documents its key formats, invariants and failure modes

What I checked in the worktree:
- **Store:** typed stores follow `TraceStore` in `traces.rs` (`for_store`, `seal`/`open`, `scan_values`). Every new column family (CF) has to be listed in four places:
  - `lib.rs::cf`
  - `expected_cf_names` in `rocks_store/mod.rs` (line 133)
  - `should_compress_column_family` (line 273)
  - `migrate.rs::SEALED_VALUE_CFS` (line 59)
- **`FaultAt` (`rocks_store/mod.rs`, line 932) only covers `append_entry_atomic`.** The new stores need their own fault hook for the atomicity tests.
- **Startup order:** `session.rs` (lines 153-159) and `headless/mod.rs` (lines 277-284) load the registry first and open the store after it.
- **`load_or_init_in` callers:**
  - `agent_registry/tests.rs`
  - `event_loop/tests.rs`
  - `event_loop/subagents/tests.rs` (lines 54 and 488)
  - `headless/tests/mod.rs`
  - `record_loader/tests.rs`
  - `tests/agent_list_perf.rs`, an integration test, so the helper must stay `pub`
- **Spawn paths:**
  - `subagents.rs` line 1346 (`open_task`)
  - `event_loop/agents.rs` line 746 (`Placement::Alone` / `InProject`)
  - `compare/run/host.rs` line 210 and `compare/run/start.rs` line 144
  - `headless/eval.rs` line 85
- **`AgentsTab::` matches:** `definitions.rs`, `keys.rs`, `mod.rs`, `context_menu.rs`, `project_drag.rs`, `archive.rs`, `panes.rs`, `renderer/panels.rs`, the tests, and also `aura-desktop/src/shell/snapshot.rs`.
- **Right column:** `side_maximized()` is `side_expanded && sandbox_panel_visible` (`app/mod.rs`, line 2572). `SidePanelTab` lives in `app/processes.rs`.

## Part 1: agents in RocksDB

### `aura-store-db/src/agents.rs`: `AgentStore`

| CF | Key | Value |
| --- | --- | --- |
| `agents` | `agent_id[32]` | `AgentRecord` (sealed) |
| `projects` | `project_id` utf-8 | `ProjectRecord` (sealed) |
| `registry_meta` | `version`, `active`, `prefixes`, `last_numbers` | JSON (sealed). `active` is `agent_id[32]` |

```rust
pub struct AgentRecord {
    pub agent_id: AgentId, pub zns_id: String, pub name: String,
    pub created_at: DateTime<Utc>, pub position: u64,
    pub definition: Option<String>, pub parent: Option<AgentId>,
    pub project_id: Option<String>, pub folder: Option<PathBuf>,
    pub archived: bool, pub removed: bool,
    pub title: Option<String>, pub code: Option<String>, pub subtitle: Option<String>,
}
pub struct RegistryDiff { pub agents_put: Vec<AgentRecord>, pub agents_del: Vec<AgentId>,
    pub projects_put: Vec<ProjectRecord>, pub projects_del: Vec<String>, pub meta: Option<RegistryMeta> }
```

- `for_store(&RocksStore)`, `load() -> RegistrySnapshot` (sorted by `position`), `apply(&RegistryDiff)` (one `WriteBatch`), and `stage(&RegistryDiff, &mut WriteBatch)`.
- `load` skips any row whose key is not `blake3(zns_id)` and logs `warn!`.
- Errors go in `AgentStoreError` (`thiserror`).
- Test hook: `#[cfg(any(test, feature = "test-support"))] apply_with_fault(diff, FaultAt)`, plus a shared `write_batch_with_fault` helper that `ChannelStore` reuses.

### Registry on top of the store (`agent_registry.rs`)

- The in-memory API and the `zns_id` parent links stay as they are. `AgentId` is converted only in `to_records()` / `from_snapshot()`.
- `save()` becomes `persist()`. It keeps a `HashMap<key, Vec<u8>>` of the serialized rows last persisted, diffs against it, and writes only changed rows in one batch.
- `position` uses gaps of 1024. A reorder rebalances the positions when the gaps run out.
- `AgentRegistry::open(store: AgentStore, data_dir, launch_dir)` is the real constructor.
- `load_or_init_in` stays `pub` and `#[doc(hidden)]`. It opens a RocksDB under `data_dir/db`, so the existing tests keep their call shape.

### Startup and import

- `session.rs` and `headless/mod.rs` open the store first, then call `AgentRegistry::open(AgentStore::for_store(&store), ..)`.
- When the `agents` CF is empty:
  - If `agents.json` exists, parse it with the existing serde types and the v0 to v3 migrations, import everything in one `apply`, then rename the file to `agents.json.migrated`.
  - Otherwise, run the existing first-run seeding.
- Rewrite `subagents/tests.rs` line 488 to edit rows through `AgentStore`.
- Rewrite `agent_list_perf.rs` to open the store once and pass it in, since RocksDB's lock rules out a second open.

## Part 2: Channels

### `aura-store-db/src/channels.rs`: `ChannelStore` (all CFs sealed)

| CF | Key | Value |
| --- | --- | --- |
| `channels` | `c/{channel_id}` | `ChannelRecord` |
| `channels` | `s/{slug}` | `channel_id` |
| `channels` | `p/{channel_id}/{project_id}` | `ProjectLink { linked_at }` |
| `channel_members` | `m/{channel_id}/` + `agent_id[32]` | `MemberRecord { role, parent, joined_at }` |
| `channel_members` | `a/` + `agent_id[32]` | `channel_id` |
| `channel_posts` | `{channel_id}/{at_ms:020}/{seq:010}` | `ChannelPost { author, text, mentions, reply_to, at_ms }` |

- **`create(NewChannel, &AgentRecord)`:** one batch holding the agent row (via `AgentStore::stage`), the channel record, the slug index, the member row and the reverse index. It rejects a slug that is already taken, checked under a store mutex.
- **Also:** `get`, `by_slug`, `list`, `update`, `archive`, `link_project`, `unlink_project`, `projects`, `add_member` (rejects an agent already in another channel), `stage_member` (for spawns), `members`, `channel_of`, `append_post`, `posts(channel, since)`.
- **Post `seq`:** an in-process `AtomicU64`, seeded on open from the last key.

### Coordinator and membership

- Add `agent_definitions/coordinator.md` and register it in the `agent_definitions.rs` seed list. It plans, delegates with `task`, and never edits files.
- **New `event_loop/channels.rs::create_channel`:**
  1. Validate the slug against `[a-z0-9-]{1,40}`.
  2. Build the coordinator with folder `<data_dir>/channels/<slug>/`.
  3. Call `ChannelStore::create` on `spawn_blocking`.
  4. Insert the coordinator into the in-memory registry and mark its row as persisted.
- **System prompt:** the coordinator gets a channel block with `#slug`, title, description, linked projects, and members with their codes.
- **Membership on spawn:** a `join_parent_channel(parent, child)` helper is called after spawning at `subagents.rs::open_task`, the two compare `create_child` sites and `headless/eval.rs`. It writes the child's agent row and member row in one batch. The role is `Agent` under the coordinator and `Subagent` otherwise. The "+ New agent" path at `agents.rs` line 746 has no parent and stays out of channels.
- **PROJECTS tree:** `record_loader::project_summaries` leaves out agents whose `channel_of` is set.

### Events (`crates/ui/aura-ui/src/events.rs`)

- **UI to app:** `CreateChannel { slug, title, description }`, `OpenChannel(id)`, `UpdateChannel { id, title, description }`, `LinkProject { channel_id, project_id }`, `UnlinkProject { .. }`, `ArchiveChannel(id)`.
- **App to UI:** `SetChannels(Vec<ChannelSummary>)`, `SetChannelTasks { channel_id, tasks }`, `ChannelPost { channel_id, post }`, `SetChannelPosts { channel_id, posts }`.
- **`ChannelSummary`:** `id`, `slug`, `title`, `description`, `coordinator_id` (hex), `projects`, and `members: Vec<AgentSummary>`, with the coordinator first and the rest nested by parent.

### Wiring

- `harness.rs` builds `AgentStore` and `ChannelStore` from the one `RocksStore`.
- `record_loader.rs` sends `SetChannels`. When a channel's chat loads, it merges posts with the coordinator's history by timestamp; on a tie, history comes first.
- `event_loop/tasks.rs::send_channel` collects the `.aura/tasks` folder of each member.

### @mentions (`event_loop/channels.rs`, called from `UiEvent::UserMessage` when the target is a coordinator)

- `@CODE` matching a member's code routes to that member. Any other `@word` is left for `attachments.rs`.
- With no member mentioned, or with `@coordinator`, the coordinator takes the turn as usual.
- When members are mentioned:
  1. Store a user post.
  2. Enqueue the message on each mentioned member's normal turn path (queued if the agent is busy), with the channel context at the top.
  3. The coordinator takes no turn unless it was mentioned too.
- A member's final reply is stored as a post with `reply_to` set and sent to the UI as `UiCommand::ChannelPost`.
- The coordinator keeps a `last_turn_ms`. Its next turn starts with a summary of posts since then, capped at about 4 KB.

## UI (`aura-ui-terminal`)

- **Tab bar:** `AgentsTab::Channels` goes first (`CHANNELS` / `CHN`). Every match arm listed above is updated, including the desktop `snapshot.rs`. The app still opens on PROJECTS.
- **Left panel:**
  - The channel list shows `#slug` rows, with "+ New channel" pinned at the bottom.
  - An open channel shows `‹ #slug`, the title, the wrapped description, then MEMBERS indented with `spawn_guides`.
- **Middle:** the chat panel is headed `#slug · coordinator`. Posts render as message blocks with the author's code. In a channel, `app/mentions.rs` lists members first, then files.
- **Right:** add `App::channel_open()` and fold it into the right-column logic without touching the user's flags:

```rust
const fn side_maximized(&self) -> bool {
    (self.side_expanded && self.sandbox_panel_visible) || self.channel_open()
}
```

  `sandbox_panel_visible()` also returns true while a channel is open. Closing the channel brings the old layout back.
- **`SidePanelTab::Projects`:** a channel-only tab, placed first and selected when the channel opens. It is rendered in the new `renderer/channel.rs` (linked projects, agents working now, and member tasks through the TASKS tree renderer).
- **Forms and menus:**
  - "+ New channel" opens a three-field form on `line_edit`, which checks the slug as you type.
  - The context menu gets "Link project" and "Unlink project".

## Tests

- **`aura-store-db`** (tempfile, plaintext and sealed):
  - Agents: round-trip and `position` order; a mismatched key is skipped; a diff writes only changed rows; `FaultAt` writes all or nothing; an existing DB opens with the new CFs.
  - Channels: atomic create, including under `FaultAt`; unique slugs; member rows stay in step with the reverse index; an agent can't join two channels; posts come back in order with `since`; project links work.
- **`aura-app`:**
  - v3 import matches the file field by field, and the file is renamed; a first run seeds as before.
  - Reorder, archive, restore and remove-project survive a reload.
  - Membership is inherited two levels deep; members are absent from PROJECTS.
  - Mention routing; reply stored as a post; unknown `@word` handled as a file mention; activity summary in the coordinator's next turn.
  - Task collection across workspaces.
  - The existing registry and `record_loader` tests still pass.
- **`aura-ui-terminal`:** tab order; tall right panel with PROJECTS selected; left panel contents; member-first picker; posts show their author; the layout comes back on close.
- **Gates:** `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test -p aura-store-db -p aura-ui -p aura-ui-terminal -p aura-app`, `cargo check -p aura-desktop`.

## Risks

- **Import parity:** keep the renamed file, and test the import field by field.
- **Startup order:** the "data directory in use" error now comes before the registry loads.
- **Registry persist blocks:** `persist()` stays synchronous under the registry mutex, replacing today's synchronous file write; the batches are small. Channel-store calls on async paths use `spawn_blocking`.
- **Sealing:** every new CF goes in `SEALED_VALUE_CFS`, and a test checks that `seal_db_copy` covers them.
- **Perf test:** it now needs a single store open. Compare it against the old JSON timings.
- **Tab matches:** these are spread across both crates. The compiler flags any that are missed, and `aura-desktop` has to compile.
