---
name: Fold subagents in a channel's MEMBERS panel
overview: In a channel's right-hand MEMBERS tab, subagents start folded under the member that spawned them. The ▸/▾ marker, Space, Right/Left and a click on the marker unfold and fold them, following the PROJECTS tree's conventions.
todos:
- id: fold-state
  content: Fold state and shown rows in ChannelState; selection, scroll and hit tests over the shown rows
  status: pending
- id: fold-input
  content: Space/Left/Right keys and a click on the marker column; reveal on select
  status: pending
- id: fold-render
  content: Render only the shown rows with a ▸/▾ marker; light the shown ancestor of a hidden active subagent
  status: pending
- id: fold-tests
  content: 'Tests: folded by default, toggling by key and click, the highlight parks and reveals, rendering; update existing tests'
  status: pending
- id: fold-ship
  content: fmt, clippy and tests for aura-ui-terminal, then commit and push
  status: pending
---

# Fold subagents in a channel's MEMBERS panel

In a channel's right-hand MEMBERS tab, subagents start folded under the member that spawned them. The ▸/▾ marker, Space, Right/Left and a click on the marker unfold and fold them, following the PROJECTS tree's conventions.

## Todos

- [ ] Fold state and shown rows in ChannelState; selection, scroll and hit tests over the shown rows
- [ ] Space/Left/Right keys and a click on the marker column; reveal on select
- [ ] Render only the shown rows with a ▸/▾ marker; light the shown ancestor of a hidden active subagent
- [ ] Tests: folded by default, toggling by key and click, the highlight parks and reveals, rendering; update existing tests
- [ ] fmt, clippy and tests for aura-ui-terminal, then commit and push

<!-- end of generated header: edit below -->

## Behavior

- **Folded by default:** a member, or a subagent, with subagents under it starts folded. Its subagents are hidden.
- **Marker:** a member with subagents shows `▸` in its icon column while folded and `▾` while open, in place of its `@`. While it works, the spinner shows instead, as in PROJECTS.
- **Unfold and fold:**
  - **Keyboard:** Space toggles the highlighted member, Right unfolds it, Left folds it. Up and Down move over the rows that are shown.
  - **Mouse:** a click on the marker column toggles. A click elsewhere on the row opens the member's chat, as now.
- **Selection:**
  - **Folding:** a highlight on a hidden row moves to the member that folded it.
  - **Lit row while unfocused:** when the chat shows a hidden subagent, its nearest shown ancestor is lit.
  - **Revealing:** selecting a member from elsewhere (the TASKS tab, or opening it) unfolds the members above it so it shows.
- **State:** the set of unfolded member ids is kept per app run in `ChannelState`. Every member starts folded and nothing is persisted.

## Code

- `app/apps/channels/fold.rs` (new): the open members, the shown rows, `has_subagents`, `toggle_channel_member`, revealing ancestors and the marker hit test.
- `app/apps/channels/{mod,side}.rs`:
  - `member_selected` stays a member index, while scroll, row hit tests and moves work over the shown rows.
  - Space, Left and Right are added to the keys.
- `renderer/channel_side.rs`: draws only the shown rows, plus the marker.
- Tests in `app/tests` and `renderer/tests`.
