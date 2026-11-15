from collections import OrderedDict


class LRUCache:
    """Fixed-size cache that evicts the least recently used key.

    Reading a key with `get` or writing it with `put` makes it the most
    recently used. A cache of capacity 0 stores nothing.
    """

    def __init__(self, capacity):
        self.capacity = capacity
        self.items = OrderedDict()

    def get(self, key):
        """Value for `key`, or None when it is not cached."""
        if key not in self.items:
            return None
        self.items.move_to_end(key)
        return self.items[key]

    def put(self, key, value):
        """Cache `value` under `key`, evicting if the cache is full."""
        if self.capacity <= 0:
            return
        self.items[key] = value
        self.items.move_to_end(key)
        while len(self.items) > self.capacity:
            self.items.popitem(last=False)
