### An Integrations panel lists every integration

##### Added

- A plug icon sits in the agents panel's header just before BENCH's
  wrench, in the terminal and the desktop app. It opens INTEGRATIONS,
  which lists every integration (GitHub, boxd, and any added later)
  with its icon, its name, a dot for its state (green connected, hollow
  not connected, red failed, amber while testing) and what it is.
- The selected integration shows how it is signed in, what it provides,
  and Connect, Configure, Test and Disconnect: click one or press `c`,
  `e`, `t` or `d`. Connect takes a key in the input, masked, and stores
  it sealed only after a passing test; a CLI login says what to run
  instead.
