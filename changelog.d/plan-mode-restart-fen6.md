### A restart mid-build carries on a build that had no plan

##### Fixed

- A channel member told to implement before it wrote a plan, then cut
  off by a restart, carries on implementing when the app comes back,
  out of plan mode and without `(P)`. Before, its build was not resumed
  and its report stayed owed until the stall sweep asked again.
