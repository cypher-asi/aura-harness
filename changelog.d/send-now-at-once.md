### Sending a queued message now starts it at once

##### Fixed

- "Send Now" on a queued message starts it as soon as the running turn
  stops. It used to wait for the agent's run to be written up after the
  turn: its scorecard, and its log rebuilt from the whole run and every
  run it spawned, which took seconds in a long conversation. Runs are
  now written up in the background, so the next queued message, and a
  spawned agent's answer to the agent that spawned it, no longer wait
  for it either. `aura runs` and the run's log still show every turn, a
  moment later.
