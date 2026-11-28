### A task typed into a channel member's chat plans first, and says so

##### Fixed

- A task you type straight into a channel member's chat now goes
  through the software factory, as one its coordinator assigns does:
  the member plans it in plan mode, then builds the plan and ships it to
  the merge queue, without waiting for approval. It used to start on the
  work at once, while its rules told it to plan first.
- The member's chat says what happens. As the planning starts it says
  "Planning first", in place of `/plan`'s note about "implement", and a
  cloud member's chat now shows plan mode too. As the build starts it
  says so. In the channel's MEMBERS, a member in plan mode has `(P)`
  after its name, as in PROJECTS.
- These run as before: "implement" or `/build`; a member you put in plan
  mode yourself, which waits for "implement"; the coordinator; an
  @mention from the channel's chat; and a message to a member on this
  computer with an unfinished plan, which carries on with it. Switching
  a planning member out of plan mode takes the task over, and the member
  does what you ask next.
