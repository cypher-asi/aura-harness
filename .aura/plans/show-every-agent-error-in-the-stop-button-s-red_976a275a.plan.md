---
name: 'Show every agent error in the Stop button''s red: rows, members and chat'
overview: '#69 already paints a failed agent''s PROJECTS row and MEMBERS row in the Stop button''s red. This adds the missing pieces: the error in the agent''s chat is red too (it is muted today), a stall and a refused start show their reason in the chat and refresh the rows, all three places share one color source with the Stop button, and tests cover every error state.'
todos:
- id: ui-color
  content: Add failure_color and use it in the Stop button, failed rows and icons, and chat error messages
  status: pending
- id: app-chat
  content: Show a stall's reason and the cloud refusals as chat errors and refresh the agent lists
  status: pending
- id: styleguide
  content: Update STYLEGUIDE.md's Failed agent entry and chat error rule
  status: pending
- id: tests
  content: Add a UI table test for the four reasons against the Stop button's color, a muted-notice test, and app tests for the stall and refusal chat errors; run fmt, clippy, tests and the size check
  status: pending
- id: ship
  content: Open the follow-up PR, merge latest main, retest, merge with a merge commit, report to COO2
  status: pending
---

# Show every agent error in the Stop button's red: rows, members and chat

#69 already paints a failed agent's PROJECTS row and MEMBERS row in the Stop button's red. This adds the missing pieces: the error in the agent's chat is red too (it is muted today), a stall and a refused start show their reason in the chat and refresh the rows, all three places share one color source with the Stop button, and tests cover every error state.

## Todos

- [ ] Add failure_color and use it in the Stop button, failed rows and icons, and chat error messages
- [ ] Show a stall's reason and the cloud refusals as chat errors and refresh the agent lists
- [ ] Update STYLEGUIDE.md's Failed agent entry and chat error rule
- [ ] Add a UI table test for the four reasons against the Stop button's color, a muted-notice test, and app tests for the stall and refusal chat errors; run fmt, clippy, tests and the size check
- [ ] Open the follow-up PR, merge latest main, retest, merge with a merge commit, report to COO2

<!-- end of generated header: edit below -->

## What already exists on main

PR #74 is merged, so this ships as a follow-up PR on `aura/error-red-rus202`, off the latest `origin/main` (`7e24f958`). I'll tell COO2 it can't go into #74 itself.

#69 added `AgentStatus` in `subagents/status.rs`. It sets `AgentSummary::error`, and the terminal draws that row's name and a `!` icon in `theme.colors.error`. A failed member's MEMBERS row shows `Failed: <why>` in the same red. The Stop button (`renderer/input/mod.rs`, `render_stop`) also uses `theme.colors.error`. These states already call `status.fail`:

- Start failure and cloud provisioning failure (`cloud/member_start.rs`, `fail_cloud_start`).
- A local turn error (`lifecycle.rs`) and a cloud turn error (`cloud/ended.rs`).
- Work that can't start (`inbox/starts.rs`).
- A stall (`stalls.rs`).

## Gaps

- **The chat error is not red.** `ShowError` adds a System message with the `error: ` prefix, and `segments.rs` (`message_style`) draws every System message in `muted`.
- **A stall says nothing in the chat.** `stalls::give_up` fails the member, but it neither shows the reason in its chat nor refreshes the agent lists, so the row turns red only on the next unrelated refresh.
- **The cloud refusals that give up work show a muted system note instead of an error.** These are the foreign-owner refusal and the plan-not-linked refusal in `cloud/submit.rs`.
- **Nothing ties the rows' red to the Stop button.** Both read `colors.error` separately, and no test checks that they match.

## Changes

- `aura-ui-terminal`: add one `failure_color(theme)` in `renderer/mod.rs`, documented as the Stop button's red. Use it in the Stop button, the failed PROJECTS row and icon, the failed MEMBERS row, and chat errors.
- `aura-ui-terminal`: draw a System message that starts with `ERROR_PREFIX` in `failure_color`, with the `✗ ` prompt, prompt and text alike. Other System messages stay muted.
- `aura-app` (stalls): `stalls::give_up` becomes async. It sends `ShowError` with the reason to the member's chat and refreshes the agent lists.
- `aura-app` (cloud refusals): the two refusals in `cloud/submit.rs` send `ShowError` instead of a muted system note, and refresh the lists when they gave up work.
- `STYLEGUIDE.md`: the Failed agent entry names the four states, says all three places (agent row, member row, chat) use the Stop button's red through `failure_color`, and says a chat error is `✗ ` in that red.

## Tests

- **UI, table-driven over the four reasons** (start, provisioning, turn, stall). For each one, the agent's PROJECTS row and icon, its MEMBERS row and its chat error are drawn in exactly the color of the Stop button's cell, read from a rendered buffer while a turn runs.
- **UI:** an ordinary System notice stays muted.
- **App:** a stalled member's chat gets the error with its reason, and its agent row carries `error`.
- **App:** a refused cloud start shows an error in the chat.

I'll run `cargo test` for `aura-ui-terminal` and `aura-app`, `fmt`, `clippy` and the file-size check. Then I'll merge the latest `origin/main`, rerun the tests, and merge with a merge commit, because FEN6, RUS197, RUS198 and RUS201 are also editing `aura-ui-terminal`.
