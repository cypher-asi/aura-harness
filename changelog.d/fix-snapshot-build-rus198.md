### A failed snapshot build says which compile error stopped it

##### Fixed

- When a cloud project's snapshot build fails, the error now quotes the
  compiler's errors instead of the last lines of its output, which were
  mostly warnings. The reason a member reports when its start fails
  also keeps that error instead of cutting it off after the command.
