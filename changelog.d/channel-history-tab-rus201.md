### Many agents no longer fill the UI's memory

##### Fixed

- With many agents at work the terminal UI held every agent's whole chat
  in memory, with each tool call's arguments and result, the lines laid
  out for it and the Record panel's rows, forever: about 1.3 MiB per
  agent after 20 turns, 250 MiB for 200 agents. Now only the four agents
  most recently on screen, and an open channel's coordinator, keep their
  chats off screen; the others let go of theirs once nothing is under
  way, and load them again in full when shown. 200 agents hold 8 MiB.
  Chats still scroll back to their start, and nothing stored is lost.
- A channel's HISTORY keeps at most 2,000 summaries in memory beside the
  channel store, and a run's trace no longer keeps every turn's cache-hit
  and blocked tool ids.

### Idle agents no longer keep their sessions for the whole run

##### Fixed

- The app kept the model conversation and kernel of every agent it had
  opened, and of every finished subagent, until it quit, archived agents
  included. It now keeps the 16 most recently left idle, saves and lets
  go of older ones that have nothing under way, and lets go of an
  archived agent's at once. They reopen from their checkpoint when the
  agent works or is shown again, with the same conversation and to-dos.
- An idle session is now kept while its agent owes reports through its
  inbox or is in a factory flow, which read its messages: before, one
  let go of then could give up the wrong owed report.

### Permanent agents are no longer parsed again on every turn

##### Changed

- Each turn re-read and parsed every permanent agent's file on the event
  loop. Now a file is parsed again only when its size or change time
  differs, so an edit on disk is still picked up at the next turn. With
  200 agents the event loop is busy 19% of the time instead of 26%.
- The load harness reports what RocksDB holds natively next to resident
  memory, and can run on mimalloc (`--features load-mimalloc`).
