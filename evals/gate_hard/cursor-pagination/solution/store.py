"""Paging through items ordered by creation time."""


def _key(item):
    return (item["created"], item["id"])


def page(items, after=None, size=3):
    """Return (items, cursor) for the page after `after`, oldest first.

    items are dicts with a unique "id" and a "created" timestamp; ties in
    "created" are ordered by "id". Pass the returned cursor as `after` to
    get the next page; None means no more pages.
    """
    ordered = sorted(items, key=_key)
    if after is not None:
        ordered = [i for i in ordered if _key(i) > tuple(after)]
    chunk = ordered[:size]
    cursor = _key(chunk[-1]) if len(ordered) > size else None
    return chunk, cursor
