### Cloud agents started together share one snapshot build

##### Fixed

- Several cloud agents started at once used to build the project's
  snapshot once each, at the same time, on a builder machine apiece.
  Now the first start builds it and the others wait and reuse it.
- A snapshot build that failed because of its machine (a compiler run
  killed for memory, a full disk, a failed download) is tried once more
  on a fresh builder. A compile error isn't retried.
- A failed build's error quotes cargo's errors, with the `Caused by:`
  lines that say a process was killed, then the last lines of the
  output.
- Three dependencies only tests use are now dev-dependencies, which
  silences cargo's `unused dependency` warnings.
