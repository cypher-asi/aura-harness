### Agents keep going until their work is done

##### Fixed

- A provider timeout (408) is retried within the turn, with the same
  backoff and cap as an overloaded or 5xx error, before the turn ends.
- An agent the model refuses for its sign-in (401, or a 403 that isn't
  Cloudflare's) no longer fails. It shows "Waiting for sign-in", keeps
  its queue and the reports it owes, and carries on by itself once a
  later model call works or you sign in again. A sign-in that is still
  refused straight after carrying on waits 2 minutes, then longer, up to
  30, before the next try.
- A member whose turn fails on an error reaching the model while it
  owes a report is asked to carry on after 1, 2 then 4 minutes. After
  the third retry it fails in red as before. Errors no retry fixes, such
  as a bad request, fail at once, and the report says why instead of
  passing on what the turn wrote before the error.
- A member whose cloud machine failed to start keeps its work queued.
  It tries again after 1, 2 then 4 minutes when the machine was at
  fault, or every 10 minutes, up to 6 times, when its project didn't
  build, so it starts once a fix lands.
