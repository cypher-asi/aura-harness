### Task branches come home with their agent

##### Fixed

- Bringing a cloud agent home (pull or recall) now fetches every branch
  that is the agent's, not only the one its machine is pinned to. That
  includes the task branches `git_start_task` made beside the pinned
  one, so unshipped work on them comes back too. Which branches count is
  the push guard's own rule (`allowed_by_pin`), so another agent's
  branches stay out.
