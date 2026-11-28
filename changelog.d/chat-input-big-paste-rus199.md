### Chat input: big pastes, a cursor that stays, Ctrl+Up/Down

##### Fixed

- A big paste (tens of thousands of characters) goes into the chat input
  whole and stays fast to edit: only the rows on screen are laid out each
  frame. A paste over 2,000 characters says how much arrived. A paste that
  would take the input past 4 MB is refused whole with a warning, and a
  clipboard past 32 MB is refused instead of pasted cut short.
- Pressing Up through a long draft no longer swaps it for an older
  message with the cursor at the bottom: on the draft's first row, Up goes
  to its start.

##### Added

- Ctrl+Up and Ctrl+Down move the cursor to the start and end of the chat
  input.
