from cache import LRUCache

c = LRUCache(2)
c.put("a", 1)
c.put("b", 2)
assert c.get("a") == 1
c.put("c", 3)
assert c.get("b") is None, "b was least recently used"
assert c.get("a") == 1 and c.get("c") == 3
c.put("a", 10)
c.put("d", 4)
assert c.get("c") is None and c.get("a") == 10 and c.get("d") == 4
z = LRUCache(0)
z.put("x", 1)
assert z.get("x") is None
