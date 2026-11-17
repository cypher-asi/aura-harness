"""Paging through items ordered by creation time."""


def page(items, after=None, size=3):
    """Return (items, cursor) for the page after `after`, oldest first.

    items are dicts with a unique "id" and a "created" timestamp. Pass the
    returned cursor as `after` to get the next page; None means no more pages.
    """
    ordered = sorted(items, key=lambda i: i["created"])
    if after is not None:
        ordered = [i for i in ordered if i["created"] > after]
    chunk = ordered[:size]
    cursor = chunk[-1]["created"] if len(ordered) > size else None
    return chunk, cursor
