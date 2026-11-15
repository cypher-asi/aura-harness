"""Token bucket limiter."""


class TokenBucket:
    """Holds up to `capacity` tokens and gains `refill_per_second` tokens a
    second. Each allowed request spends `cost` tokens."""

    def __init__(self, capacity: float, refill_per_second: float, clock) -> None:
        if capacity <= 0:
            raise ValueError("capacity must be positive")
        if refill_per_second <= 0:
            raise ValueError("refill_per_second must be positive")
        self.capacity = float(capacity)
        self.refill_per_second = float(refill_per_second)
        self._clock = clock
        self._tokens = float(capacity)
        self._updated = clock.now()

    def _refill(self) -> None:
        now = self._clock.now()
        elapsed = now - self._updated
        self._tokens = min(self.capacity, self._tokens + elapsed * self.refill_per_second)
        self._updated = now

    @property
    def tokens(self) -> float:
        self._refill()
        return self._tokens

    def allow(self, cost: float = 1) -> bool:
        self._refill()
        if cost <= self._tokens:
            self._tokens -= cost
            return True
        return False
