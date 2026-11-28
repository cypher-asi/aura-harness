### Plan mode ends when implementation starts

##### Fixed

- A channel member still planning its task that is told to implement
  ("implement", "build it", `/build`) leaves plan mode and builds, and
  its build is shipped and reported as a planned one is. Before, it
  planned on in plan mode.
- A cloud turn asked to plan that starts editing, committing or pushing
  takes its agent out of plan mode from that call on, since a cloud
  machine doesn't hold it to plan mode.
- Every row an agent is on (PROJECTS, a channel's MEMBERS) drops its
  `(P)` the moment it leaves plan mode, not at the next list refresh.
