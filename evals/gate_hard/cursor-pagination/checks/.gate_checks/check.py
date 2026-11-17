import random
from store import page

created = [1, 2, 2, 2, 2, 3, 3, 4, 5, 5]
ids = list(range(100, 110))
random.Random(7).shuffle(ids)
items = [{"id": i, "created": c} for i, c in zip(ids, created)]
want = [i["id"] for i in sorted(items, key=lambda i: (i["created"], i["id"]))]
for size in range(1, 12):
    shuffled = items[:]
    random.Random(size).shuffle(shuffled)
    seen, cursor, pages = [], None, 0
    while True:
        chunk, cursor = page(shuffled, after=cursor, size=size)
        seen += [i["id"] for i in chunk]
        pages += 1
        assert pages < 50, "paging never ends"
        if cursor is None:
            break
    assert seen == want, (size, seen, want)
assert page([], size=3) == ([], None)
print("ok")
