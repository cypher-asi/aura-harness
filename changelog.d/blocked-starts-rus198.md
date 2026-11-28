### A member whose project doesn't build isn't started again for every message

##### Fixed

- When a channel member's start on a cloud machine fails for a reason a
  new try won't fix, such as its project not compiling, messages sent to
  it no longer each start a new 12-minute build that fails the same way.
  The member stays failed, in red, and each message is turned away at
  once with the reason. A message you type to it yourself retries it,
  and so does work after 10 minutes. If the merge target still has the
  same broken commit, that later try fails in seconds without a builder.
  A start that fails because of its machine (a timeout, a killed build,
  a full disk, no connection) is still tried again by the next work.
