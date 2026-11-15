"""Sliding window limiter."""

from collections import deque


class SlidingWindowLimiter:
    def __init__(self, limit: float, window_seconds: float, clock) -> None:
        if limit <= 0:
            raise ValueError("limit must be positive")
        if window_seconds <= 0:
            raise ValueError("window_seconds must be positive")
        self.limit = limit
        self.window_seconds = window_seconds
        self._clock = clock
        self._events = deque()

    def _expire(self, now: float) -> None:
        while self._events and now - self._events[0][0] >= self.window_seconds:
            self._events.popleft()

    def _used(self) -> float:
        return sum(cost for _, cost in self._events)

    def allow(self, cost: float = 1) -> bool:
        now = self._clock.now()
        self._expire(now)
        if self._used() + cost > self.limit:
            return False
        self._events.append((now, cost))
        return True

    def retry_after(self, cost: float = 1) -> float:
        if cost > self.limit:
            raise ValueError("cost exceeds the limit")
        now = self._clock.now()
        self._expire(now)
        used = self._used()
        if used + cost <= self.limit:
            return 0.0
        for at, spent in self._events:
            used -= spent
            if used + cost <= self.limit:
                return max(at + self.window_seconds - now, 0.0)
        return 0.0
