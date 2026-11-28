### A cloud turn whose conversation can't be copied says so

##### Fixed

- When a cloud agent's conversation can't be copied back from its
  machine at the end of a turn, its chat now shows a muted note saying
  so and why, and the log has a warning with the agent's code. It used
  to be logged only at debug level, so nothing showed. The turn's
  report then uses the reply it streamed, not the earlier turn's
  conversation still on this computer.
