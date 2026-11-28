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
