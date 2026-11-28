### The software factory survives a restart

##### Fixed

- A channel member that was planning, building or shipping when the app
  closed carries on at the same stage when it reopens. It plans again in
  plan mode, builds its plan, or is asked to ship, and its report then
  reaches the coordinator. Before, it lost its place: it could leave its
  plan unbuilt or never ship, and the report it was holding back never
  arrived.

### Updating an old cloud machine never cuts off a turn

##### Fixed

- The hub no longer restarts an old cloud machine's node to update it
  while a turn may be running there. It asks the node first, so turns
  the node started itself (for mail, or a spawned agent) count too. It
  keeps a turn's mark when it loses track of the turn before seeing it
  end. Two turns starting at once update the node only once.

### Task records stay out of the repository

##### Changed

- `.aura/tasks/*.json`, the task records the app writes for the TASKS
  tab, are no longer tracked: they are ignored and were removed from
  the index. The files stay on disk, so the TASKS tab is unchanged.

### Restarting reconnects to running cloud turns

##### Fixed

- When the app restarts while cloud agents work, it reconnects to their
  running machines instead of waking them. Their chat and channel row
  say "Reconnecting…", not "Waking cloud machine". The agent on screen
  is reconnected first, at most eight at a time, in the background. A
  machine that can't be reached is tried again with backoff before its
  turn is reported lost. A sleeping machine with no work is left asleep.
