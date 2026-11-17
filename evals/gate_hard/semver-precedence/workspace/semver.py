"""Compare semantic version strings."""
import functools


def parse(v):
    core, _, pre = v.partition("-")
    major, minor, patch = (int(x) for x in core.split("."))
    return (major, minor, patch, pre)


def compare(a, b):
    """Return -1, 0 or 1 as version a is lower than, equal to or higher than b."""
    pa, pb = parse(a), parse(b)
    if pa[:3] != pb[:3]:
        return -1 if pa[:3] < pb[:3] else 1
    if pa[3] == pb[3]:
        return 0
    if not pa[3]:
        return 1
    if not pb[3]:
        return -1
    return -1 if pa[3] < pb[3] else 1


def sort_versions(versions):
    return sorted(versions, key=functools.cmp_to_key(compare))
