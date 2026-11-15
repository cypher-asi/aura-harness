"""Limiters from configuration."""

from .sliding_window import SlidingWindowLimiter
from .token_bucket import TokenBucket

KINDS = {
    "token_bucket": (TokenBucket, ("capacity", "refill_per_second")),
    "sliding_window": (SlidingWindowLimiter, ("limit", "window_seconds")),
}


def limiter_factory(config, clock):
    if "type" not in config:
        raise ValueError("missing key: type")
    kind = config["type"]
    if kind not in KINDS:
        raise ValueError(f"unknown limiter type: {kind}")
    cls, keys = KINDS[kind]
    for key in keys:
        if key not in config:
            raise ValueError(f"missing key: {key}")
    for key in config:
        if key != "type" and key not in keys:
            raise ValueError(f"unexpected key: {key}")
    args = [config[key] for key in keys]
    return lambda: cls(*args, clock)
