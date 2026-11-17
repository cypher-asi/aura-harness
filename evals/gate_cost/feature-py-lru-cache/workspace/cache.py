class LRUCache:
    """Fixed-size cache that evicts the least recently used key.

    Reading a key with `get` or writing it with `put` makes it the most
    recently used. A cache of capacity 0 stores nothing.
    """

    def __init__(self, capacity):
        raise NotImplementedError

    def get(self, key):
        """Value for `key`, or None when it is not cached."""
        raise NotImplementedError

    def put(self, key, value):
        """Cache `value` under `key`, evicting if the cache is full."""
        raise NotImplementedError
