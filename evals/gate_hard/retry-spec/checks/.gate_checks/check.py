from retry import retry


class Clock:
    def __init__(self):
        self.t = 0.0
        self.sleeps = []

    def clock(self):
        return self.t

    def sleep(self, s):
        self.sleeps.append(round(s, 9))
        self.t += s


def failing(times, clock=None, cost=0.0, exc=ValueError):
    calls = []

    def fn():
        calls.append(1)
        if clock:
            clock.t += cost
        if len(calls) <= times:
            raise exc(len(calls))
        return "ok"

    return fn, calls


c = Clock()
fn, calls = failing(2)
assert retry(fn, attempts=5, base=1, cap=10, deadline=100, sleep=c.sleep, clock=c.clock, rand=lambda: 0.5) == "ok"
assert c.sleeps == [0.5, 1.0], c.sleeps
assert len(calls) == 3

c = Clock()
fn, calls = failing(99)
try:
    retry(fn, attempts=6, base=1, cap=4, deadline=1000, sleep=c.sleep, clock=c.clock, rand=lambda: 0.5)
except ValueError as e:
    assert e.args == (6,), e.args
else:
    raise AssertionError("should raise")
assert c.sleeps == [0.5, 1.0, 2.0, 2.0, 2.0], c.sleeps
assert len(calls) == 6

c = Clock()
fn, calls = failing(99, clock=c, cost=1.0)
try:
    retry(fn, attempts=10, base=2, cap=100, deadline=13, sleep=c.sleep, clock=c.clock, rand=lambda: 0.75)
except ValueError as e:
    assert e.args == (3,), e.args
else:
    raise AssertionError("should raise")
assert c.sleeps == [1.5, 3.0], c.sleeps
assert len(calls) == 3

c = Clock()
fn, calls = failing(1, exc=KeyError)
try:
    retry(fn, attempts=5, base=1, cap=10, deadline=100, retry_on=(ValueError,), sleep=c.sleep, clock=c.clock, rand=lambda: 0.5)
except KeyError:
    pass
else:
    raise AssertionError("KeyError should propagate")
assert c.sleeps == [] and len(calls) == 1

c = Clock()
fn, calls = failing(5)
try:
    retry(fn, attempts=1, base=1, cap=10, deadline=100, sleep=c.sleep, clock=c.clock, rand=lambda: 0.5)
except ValueError:
    pass
else:
    raise AssertionError("should raise")
assert c.sleeps == [] and len(calls) == 1

rolls = iter([0.1, 0.9, 0.3])
c = Clock()
fn, calls = failing(3)
assert retry(fn, attempts=4, base=3, cap=50, deadline=100, sleep=c.sleep, clock=c.clock, rand=lambda: next(rolls)) == "ok"
assert c.sleeps == [0.3, 5.4, 3.6], c.sleeps
print("ok")
