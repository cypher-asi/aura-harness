### A member's row shows what it does now, also after a failure

##### Fixed

- A channel member that failed and starts again no longer keeps its red
  `Failed:` row while it starts. The row now reads what the member's
  peers hear: from the moment the new start begins it says
  `Starting 5/9 · 1m 26s · Preparing project snapshot`, the step its
  chat's start panel is at, then waiting, working or idle. A new failure
  shows its own reason.
- Pointing at a member's row, or selecting it, shows the whole failure or
  start step the row cuts off.
