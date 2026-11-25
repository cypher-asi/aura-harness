---
name: Haiku subjects for INBOX threads
overview: INBOX threads whose subject is just the first line of the opening prompt (every `task` thread, plus host threads opened without a `subject`) get a short subject written by the title model, the same Haiku call that names chats and subtasks. The UI swaps it in as soon as it arrives.
todos:
- id: prompt
  content: Add mail subject system/user prompts in chat_title.rs with a test
  status: completed
- id: titles
  content: Add TitleKind::Subject, ask() split and spawn_subject in titles.rs
  status: completed
- id: ui-cmd
  content: Add UiCommand::SetThreadSubject and route it in terminal commands.rs
  status: completed
- id: inbox
  content: Keep per-thread subject overrides in Inbox; update rows, open view, eviction; tests
  status: completed
- id: engine-flag
  content: Flag text-derived thread openers on MailMessage/Envelope in aura-engine; tests
  status: completed
- id: mail-item
  content: Carry MailItem{view,title_from} on the app mail channel; Mailbox sets it
  status: completed
- id: task-trigger
  content: Title task threads from name_subtask on first attempt (skip bench/no mail)
  status: completed
- id: host-trigger
  content: Add AgentHub::title_thread and call it from the mail_rx arm in mod.rs
  status: completed
- id: verify
  content: Run fmt, clippy -D warnings and tests for touched crates
  status: completed
---

# Haiku subjects for INBOX threads

INBOX threads whose subject is just the first line of the opening prompt (every `task` thread, plus host threads opened without a `subject`) get a short subject written by the title model, the same Haiku call that names chats and subtasks. The UI swaps it in as soon as it arrives.

## Todos

- [x] Add mail subject system/user prompts in chat_title.rs with a test
- [x] Add TitleKind::Subject, ask() split and spawn_subject in titles.rs
- [x] Add UiCommand::SetThreadSubject and route it in terminal commands.rs
- [x] Keep per-thread subject overrides in Inbox; update rows, open view, eviction; tests
- [x] Flag text-derived thread openers on MailMessage/Envelope in aura-engine; tests
- [x] Carry MailItem{view,title_from} on the app mail channel; Mailbox sets it
- [x] Title task threads from name_subtask on first attempt (skip bench/no mail)
- [x] Add AgentHub::title_thread and call it from the mail_rx arm in mod.rs
- [x] Run fmt, clippy -D warnings and tests for touched crates

<!-- end of generated header: edit below -->

## Why the rows look like that

Today an INBOX subject only names the thread well when an agent passes `subject` to `send_to_agent`. Every other thread is named after the first line of its opening message:

- `task` threads: `TaskMail::new` -> `task_subject(prompt)` in `crates/surface/aura-app/src/event_loop/mail.rs`. These are the `(2)` rows in the screenshot (request plus reply), all starting "Work ONLY in /home/n3o/code/...".
- Host threads with no subject (`spawn_agent` first prompts, delegated tasks, `send_to_agent` with no subject and no earlier thread): `Threads::new_thread` -> `derived_subject(text)` in `crates/engine/aura-engine/src/agents/mail.rs`.

Chats and subtasks already get names from the title model (`AURA_TITLE_MODEL`, which defaults to the Gate classifier, Haiku) in `event_loop/titles.rs`. The plan reuses that path for thread subjects.

## Approach

1. **New title kind.** Add `TitleKind::Subject` to `titles.rs` with its own prompt and length limit (about 48 characters). The call runs in the background, the same way `spawn_subtitle` does. If the call fails or times out, nothing is sent and the subject made from the first line stays.
2. **A new UI command, so the subject can change after the mail arrives.** `UiCommand::SetThreadSubject { thread_id, subject }`. The terminal INBOX keeps one replacement subject per thread. The thread row and the open thread view use it ahead of `MailView::subject`. It doesn't matter whether the title or the mail arrives first, and later messages that still carry the old subject don't undo it.
3. **Two triggers:**
   - `task` threads: when the first attempt opens (`open_task` with `escalation == None`), title thread `task_id(child)` from the prompt through the child's kernel, next to the subtitle call. Retries reuse the first instance's thread, so they don't title it again.
   - Host threads: the engine marks the message that opened a thread with a subject made from its text. The app titles that thread through the sender's kernel.
4. **When no title call is made:**
   - Benchmark runs (`in_bench`), to match the rule that their agents are named without model calls.
   - Surfaces that never open mail (`lock(&self.mail).is_none()`, e.g. `aura -p`).
   - Threads that already have an explicit subject.

```rust
// aura-ui/src/events.rs
/// A better subject for a thread, written after its first message.
SetThreadSubject { thread_id: String, subject: String },
```

## Files to change

| File | Change |
|---|---|
| `crates/context/aura-context-prompts/src/auxiliary/chat_title.rs` | Add `MAIL_SUBJECT_SYSTEM_PROMPT` (an email-style subject of 3 to 6 words in sentence case, e.g. "Inbox subject titles", "Review parser escapes"; ignore boilerplate like workspace paths and "Work ONLY in"; no quotes or trailing punctuation) and `mail_subject_user_prompt(text)`. Add a unit test. |
| `crates/surface/aura-app/src/event_loop/titles.rs` | Add the `TitleKind::Subject` variant (prompt, `MAX_SUBJECT_CHARS`). Split `generate_kind` into `ask(kind, ..) -> Option<String>` plus the existing fallback wrapper. Add `spawn_subject(commands, thread_id, gateway, config, model, text)`, which sends `SetThreadSubject` only on `Some`. Add tests: the subject prompt is used, and a failed or silent model sends nothing. |
| `crates/ui/aura-ui/src/events.rs` | Add the `UiCommand::SetThreadSubject` variant. |
| `crates/ui/aura-ui-terminal/src/app/inbox.rs` | Add `Inbox.subjects: HashMap<String, String>`. `thread_row` and `open_thread` use it first. Add `set_thread_subject`, which also updates an open `ThreadView.subject` and calls `refresh_inbox`. Drop entries for evicted threads next to `expanded`/`read` in `Inbox::add`. Tests: retitle before and after mail, open view header, eviction. |
| `crates/ui/aura-ui-terminal/src/app/commands.rs` | Route the new command to `self.set_thread_subject(..)`. |
| `crates/engine/aura-engine/src/agents/mail.rs` | `Threads::resolve`/`new_thread` also report whether the subject was made from the text. Add `pub subject_from_text: bool` on `MailMessage`. Update tests. |
| `crates/engine/aura-engine/src/agents/host.rs` | Carry the flag on `Envelope` (`envelope()` sets it; `deliver_remote` and reply/cancel mail set `false`) and copy it in `unsent_mail`. Add a host test: the first subjectless message is flagged, a later one or an explicit subject is not. |
| `crates/surface/aura-app/src/event_loop/mail.rs` | The mail channel carries `MailItem { view, title_from: Option<AgentId> }`. `Mailbox::on_mail` sets `title_from = Some(from)` when `subject_from_text`. `TaskMail` posts with `None`. Update tests. |
| `crates/surface/aura-app/src/event_loop/subagents.rs` | Give `name_subtask` an extra `thread: Option<String>`; `open_task` passes `escalation.is_none().then(|| task_id(child_id))`, and it calls `titles::spawn_subject` beside `spawn_subtitle`. Add `AgentHub::title_thread(sender, thread_id, text)` for host mail: `running_kernel(sender)`, credentials from `self.turn(None, &kernel.agent_workspace())`, skipped in bench or when no kernel is running. `post_mail` takes a `MailItem`. |
| `crates/surface/aura-app/src/event_loop/mod.rs` | In the `mail_rx` arm, forward `item.view` and call `state.hub.title_thread(..)` when `title_from` is set. |
| `crates/surface/aura-app/src/event_loop/subagents/tests.rs` | Add a test: a spawned task's INBOX thread gets `SetThreadSubject` with the mock title model's answer, and a bench run gets none. |

The desktop router needs no change: the new variant goes through its `other =>` arm, the same as `AddMail`.

## Risks

- **Cost:** one more small Haiku call per `task` spawn and per new host thread. It is bounded by `TITLE_TIMEOUT` (15 s) and is skipped in benchmarks and headless runs.
- **Privacy:** log only titles the model wrote, never the prompt, as `generate_kind` does today.
- **Persistence:** subjects live only in the UI, like the inbox itself. The engine's `by_subject` index keeps the key made from the text, so explicit `re:` replies to the old subject still join the thread.

## Verification

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test -p aura-engine -p aura-app -p aura-ui-terminal -p aura-context-prompts
```
