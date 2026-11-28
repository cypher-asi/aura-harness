### The merge queue no longer fails an entry with "cannot lock ref"

##### Fixed

- A push could fail in the merge queue with `git fetch: cannot lock ref
  'refs/remotes/origin/<branch>'`, though nothing was wrong with it. When
  a push is queued, the queue reads which files it changes while it may
  already be fetching the same branch to merge it. Both fetches also
  moved the hub's `origin/<branch>`, and the one that lost the race for
  it failed the entry. The queue's fetches now write only its own refs,
  so they can't collide, and the hub's `origin/*` branches stay as the
  hub's own fetches left them.
