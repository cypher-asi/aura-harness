### To-do drawer items take two rows, like the side panel's members

##### Changed

- Each item in the to-do drawer now takes two rows, as a member does on
  the side panel: its text on top, then in gray the agent it was given
  to and how that agent is doing (`RUS198 · working 4m`), or the item's
  state when no agent has it. Before, the agent sat at the end of the
  item's one row and cut its text short.
- Nothing in the drawer is bold any more. As on the side panel, an
  item's text is white while it is open and gray once it is closed, and
  only its marker shows its state.

##### Fixed

- A long to-do list no longer leaves a blank row under its `+N more`
  row.
