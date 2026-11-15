import unittest

from ratelimit import LimiterRegistry, ManualClock, SlidingWindowLimiter, TokenBucket, limiter_factory


class HiddenTokenBucketTest(unittest.TestCase):
    def test_retry_after(self):
        clock = ManualClock()
        bucket = TokenBucket(capacity=2, refill_per_second=0.5, clock=clock)
        self.assertEqual(bucket.retry_after(), 0.0)
        bucket.allow(2)
        self.assertAlmostEqual(bucket.retry_after(), 2.0)
        self.assertAlmostEqual(bucket.retry_after(2), 4.0)
        self.assertAlmostEqual(bucket.tokens, 0.0)
        clock.advance(1)
        self.assertAlmostEqual(bucket.retry_after(), 1.0)
        with self.assertRaises(ValueError):
            bucket.retry_after(3)


class HiddenSlidingWindowTest(unittest.TestCase):
    def test_window(self):
        clock = ManualClock()
        limiter = SlidingWindowLimiter(limit=3, window_seconds=10, clock=clock)
        self.assertTrue(limiter.allow())
        clock.advance(4)
        self.assertTrue(limiter.allow(2))
        self.assertFalse(limiter.allow())
        self.assertAlmostEqual(limiter.retry_after(), 6.0)
        self.assertAlmostEqual(limiter.retry_after(3), 10.0)
        clock.advance(5.5)
        self.assertFalse(limiter.allow())
        clock.advance(0.5)
        self.assertTrue(limiter.allow())
        self.assertFalse(limiter.allow())

    def test_denied_requests_are_not_recorded(self):
        clock = ManualClock()
        limiter = SlidingWindowLimiter(limit=1, window_seconds=1, clock=clock)
        self.assertTrue(limiter.allow())
        for _ in range(5):
            self.assertFalse(limiter.allow())
        clock.advance(1)
        self.assertEqual(limiter.retry_after(), 0.0)
        self.assertTrue(limiter.allow())

    def test_bad_settings(self):
        clock = ManualClock()
        with self.assertRaises(ValueError):
            SlidingWindowLimiter(limit=0, window_seconds=1, clock=clock)
        with self.assertRaises(ValueError):
            SlidingWindowLimiter(limit=1, window_seconds=0, clock=clock)
        with self.assertRaises(ValueError):
            SlidingWindowLimiter(limit=2, window_seconds=1, clock=clock).retry_after(3)


class HiddenRegistryTest(unittest.TestCase):
    def test_clients_are_independent_and_idle_ones_forgotten(self):
        clock = ManualClock()
        registry = LimiterRegistry(lambda: TokenBucket(1, 1, clock), clock)
        self.assertTrue(registry.allow("a"))
        self.assertFalse(registry.allow("a"))
        self.assertTrue(registry.allow("b"))
        self.assertEqual(len(registry), 2)
        self.assertAlmostEqual(registry.retry_after("a"), 1.0)

        clock.advance(30)
        registry.allow("b")
        clock.advance(30)
        self.assertEqual(registry.forget_idle(60), 1)
        self.assertEqual(len(registry), 1)
        self.assertEqual(registry.forget_idle(30), 1)
        self.assertEqual(len(registry), 0)


class HiddenConfigTest(unittest.TestCase):
    def test_builds_both_kinds(self):
        clock = ManualClock()
        bucket = limiter_factory({"type": "token_bucket", "capacity": 2, "refill_per_second": 1}, clock)()
        self.assertIsInstance(bucket, TokenBucket)
        self.assertEqual(bucket.capacity, 2)
        window = limiter_factory({"type": "sliding_window", "limit": 5, "window_seconds": 60}, clock)()
        self.assertIsInstance(window, SlidingWindowLimiter)
        f = limiter_factory({"type": "sliding_window", "limit": 1, "window_seconds": 60}, clock)
        self.assertIsNot(f(), f())

    def test_errors_name_the_problem(self):
        clock = ManualClock()
        cases = [
            ({"type": "leaky", "capacity": 1}, "leaky"),
            ({"type": "token_bucket", "capacity": 1}, "refill_per_second"),
            ({"type": "sliding_window", "limit": 1, "window_seconds": 1, "burst": 2}, "burst"),
            ({"limit": 1}, "type"),
        ]
        for config, word in cases:
            with self.subTest(config=config):
                with self.assertRaises(ValueError) as caught:
                    limiter_factory(config, clock)
                self.assertIn(word, str(caught.exception))


if __name__ == "__main__":
    unittest.main()
