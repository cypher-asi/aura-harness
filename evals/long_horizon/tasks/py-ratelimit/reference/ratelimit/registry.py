"""One limiter per client."""


class LimiterRegistry:
    def __init__(self, factory, clock) -> None:
        self._factory = factory
        self._clock = clock
        self._limiters = {}
        self._last_used = {}

    def _limiter(self, client_id):
        limiter = self._limiters.get(client_id)
        if limiter is None:
            limiter = self._limiters[client_id] = self._factory()
        return limiter

    def allow(self, client_id, cost=1) -> bool:
        self._last_used[client_id] = self._clock.now()
        return self._limiter(client_id).allow(cost)

    def retry_after(self, client_id, cost=1) -> float:
        return self._limiter(client_id).retry_after(cost)

    def forget_idle(self, idle_seconds: float) -> int:
        now = self._clock.now()
        idle = [c for c, at in self._last_used.items() if now - at >= idle_seconds]
        idle += [c for c in self._limiters if c not in self._last_used]
        for client_id in idle:
            self._limiters.pop(client_id, None)
            self._last_used.pop(client_id, None)
        return len(idle)

    def __len__(self) -> int:
        return len(self._limiters)
