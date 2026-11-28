### A running turn's steps stay folded until you open them

##### Fixed

- While an agent works, its chat shows only the running group's summary
  line, such as "Running 28 commands · step 46 · 3m 19s". It used to
  list every step with a few lines of what each thought or printed,
  which filled the chat of cloud and spawned agents during their long
  turns. Click the summary, or press Ctrl+O, to see the steps and their
  output; the same folds them again, and a click is remembered while the
  chat is open. This holds for local and cloud agents, and for a turn
  the chat picks up again after a restart.
