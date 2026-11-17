"""Tiny memoizer used across the service.

The rate lookups behind these functions are slow remote calls, so
everything memoized must stay cached.
"""
import functools


def memoize(fn):
    store = {}

    @functools.wraps(fn)
    def wrapper(*args, **kwargs):
        key = (args, tuple(sorted(kwargs.items())))
        if key not in store:
            store[key] = fn(*args, **kwargs)
        return store[key]

    wrapper.cache_clear = store.clear
    return wrapper
