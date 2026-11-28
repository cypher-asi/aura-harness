### Channels: tasks open their agent, finished to-dos list, INFO, QUEUE and HISTORY read alike, and lists breathe on the desktop

##### Fixed

- Opening "Finished to-dos" listed nothing but "29+ more" when four or
  more items were still open: the drawer always started at the item in
  progress and stopped at ten rows, so the open items filled them. An
  opened history now starts at its own row, each finished item shows
  with its agent and how it ended, and the wheel over the drawer scrolls
  a long one.
- A click or Enter on a TASKS row of a channel's to-dos opened the
  coordinator already on screen. It now opens the agent the row was
  given to (a plan todo's task's agent, else its assignee), as the
  member's chat in the channel; a row given to no agent opens nothing.
  A click on an item of the to-do drawer does the same.

##### Changed

- A channel's INFO opens with a `CHANNEL` section, its slug, title and
  description indented under it like every other section, and nothing
  bold.
- QUEUE draws each merge as a section headed by a colored status dot,
  its state and short sha, with the branch on its own line; a click on
  the branch opens it on GitHub when the repo is there. No text on the
  tab is bold or colored. The dot is a shared `StatusDot`.
- HISTORY is drawn like MEMBERS: the kind of message in the icon
  column, the message, then a muted row with its time and where it went
  (`to members`, `to the coordinator`, `plan · RUS3`). The bold heading
  is gone. The second row is MEMBERS' own, now shared as `detail_row`.
- In aura-desktop, single-line lists (CHANNELS, PROJECTS, AGENTS,
  ARCHIVE, BENCH and the side and Kernel panels' lists) have rows 1.3
  rows tall, on whole pixels; the pointer follows them and menus over
  them stay put. A terminal keeps one cell per row: whole cells can only
  give +0% or +100%.
