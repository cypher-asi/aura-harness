"""Compare semantic version strings."""
import functools


def parse(v):
    v = v.split("+", 1)[0]
    core, _, pre = v.partition("-")
    major, minor, patch = (int(x) for x in core.split("."))
    return (major, minor, patch), (pre.split(".") if pre else [])


def _compare_identifier(x, y):
    x_num, y_num = x.isdigit(), y.isdigit()
    if x_num and y_num:
        return (int(x) > int(y)) - (int(x) < int(y))
    if x_num:
        return -1
    if y_num:
        return 1
    return (x > y) - (x < y)


def compare(a, b):
    """Return -1, 0 or 1 as version a is lower than, equal to or higher than b."""
    (core_a, pre_a), (core_b, pre_b) = parse(a), parse(b)
    if core_a != core_b:
        return -1 if core_a < core_b else 1
    if pre_a == pre_b:
        return 0
    if not pre_a:
        return 1
    if not pre_b:
        return -1
    for x, y in zip(pre_a, pre_b):
        c = _compare_identifier(x, y)
        if c:
            return c
    return (len(pre_a) > len(pre_b)) - (len(pre_a) < len(pre_b))


def sort_versions(versions):
    return sorted(versions, key=functools.cmp_to_key(compare))
