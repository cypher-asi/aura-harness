import unittest

from ratelimit import ManualClock, TokenBucket


class TokenBucketTest(unittest.TestCase):
    def test_spends_and_refills(self):
        clock = ManualClock()
        bucket = TokenBucket(capacity=2, refill_per_second=1, clock=clock)
        self.assertTrue(bucket.allow())
        self.assertTrue(bucket.allow())
        self.assertFalse(bucket.allow())
        clock.advance(1)
        self.assertTrue(bucket.allow())

    def test_never_exceeds_capacity(self):
        clock = ManualClock()
        bucket = TokenBucket(capacity=3, refill_per_second=10, clock=clock)
        clock.advance(100)
        self.assertEqual(bucket.tokens, 3)

    def test_cost(self):
        bucket = TokenBucket(capacity=5, refill_per_second=1, clock=ManualClock())
        self.assertTrue(bucket.allow(cost=4))
        self.assertFalse(bucket.allow(cost=2))

    def test_rejects_bad_settings(self):
        with self.assertRaises(ValueError):
            TokenBucket(capacity=0, refill_per_second=1, clock=ManualClock())


if __name__ == "__main__":
    unittest.main()
