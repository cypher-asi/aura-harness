"""Retry with exponential backoff."""
import random
import time


def retry(fn, *, attempts, base, cap, deadline, retry_on=(Exception,), sleep=None, clock=None, rand=None):
    """Call fn() until it succeeds; see README.md for the rules."""
    sleep = sleep or time.sleep
    clock = clock or time.monotonic
    rand = rand or random.random
    start = clock()
    for n in range(attempts):
        try:
            return fn()
        except retry_on:
            if n == attempts - 1:
                raise
            delay = rand() * min(cap, base * 2**n)
            if clock() + delay > start + deadline:
                raise
            sleep(delay)
    raise AssertionError("attempts must be at least 1")
