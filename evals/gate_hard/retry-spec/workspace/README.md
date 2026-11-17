# retry()

`retry(fn, *, attempts, base, cap, deadline, retry_on=(Exception,), sleep=None, clock=None, rand=None)`

- Call `fn()` and return its result as soon as a call succeeds.
- An exception that is not an instance of one of `retry_on` propagates at once.
- `attempts` is the most calls to make (at least 1). When the last allowed call fails, raise
  that call's exception.
- Before retry number `n` (`n` is 0 before the second call, 1 before the third, ...), the time
  to sleep is `rand() * min(cap, base * 2**n)`.
- `deadline` is in seconds from the start, where the start is `clock()` read once just before
  the first call. If `clock()` plus the time to sleep would be later than start + `deadline`,
  do not sleep: raise the last exception.
- `sleep`, `clock` and `rand` default to `time.sleep`, `time.monotonic` and `random.random`.
