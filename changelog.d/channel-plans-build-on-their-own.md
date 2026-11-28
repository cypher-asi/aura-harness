### A channel's agents build their plans without asking you to start

##### Fixed

- A channel member planning its task no longer ends by asking you to
  say "implement" or run `/build`. Its plan-mode instructions now say
  the plan is built right after the turn, with nobody to approve it, so
  it asks a question only when a wrong guess could cause harm. A member
  on this computer then says "Plan ready: … It is built next, without
  waiting for approval."
- `/plan`'s planner works the same way: it takes your request as its
  task, writes the plan, then builds it and ships it to the merge queue
  on its own. Bare `/plan` opens it waiting for the request, as before.
- A member you put in plan mode yourself, with `/plan` or `/mode plan`
  in its own chat, still waits for "implement" or `/build`.
