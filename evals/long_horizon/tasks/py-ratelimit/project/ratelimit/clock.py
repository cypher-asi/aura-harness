"""Time sources. Limiters take a clock so tests can control time."""

import time


class SystemClock:
    def now(self) -> float:
        return time.monotonic()


class ManualClock:
    def __init__(self, start: float = 0.0) -> None:
        self._now = start

    def now(self) -> float:
        return self._now

    def advance(self, seconds: float) -> None:
        if seconds < 0:
            raise ValueError("time only moves forward")
        self._now += seconds
