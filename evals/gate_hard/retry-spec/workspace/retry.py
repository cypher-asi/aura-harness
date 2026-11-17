"""Retry with exponential backoff."""


def retry(fn, *, attempts, base, cap, deadline, retry_on=(Exception,), sleep=None, clock=None, rand=None):
    """Call fn() until it succeeds; see README.md for the rules."""
    raise NotImplementedError
