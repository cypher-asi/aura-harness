### Cloud snapshots build from what is shipped

##### Fixed

- A project's cloud snapshot now builds from `origin`'s merge target,
  fetched just before, instead of this computer's checkout. A stale
  local checkout, or one with unpushed commits that don't compile, no
  longer blocks every new cloud agent. When the checkout has commits the
  snapshot leaves out, or there is no `origin` to build from, the start
  says so as a warning.
- A channel member's machine starts from `origin`'s target as this
  computer has it, even when the machine can't fetch it.
- A snapshot build that failed with a compile error isn't built again at
  the same commit on a new builder for every start. Starts fail at once
  with that error until the target moves or its lockfiles change.
