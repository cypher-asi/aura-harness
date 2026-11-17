import math


def circle_area(radius):
    """Area of a circle with the given `radius`."""
    return math.pi * radius ** 2


def rect_area(width, height):
    """Area of a `width` by `height` rectangle."""
    return width * height


def distance(a, b):
    """Euclidean distance between points `a` and `b`, each (x, y)."""
    return math.hypot(b[0] - a[0], b[1] - a[1])


def centroid(points):
    """Mean (x, y) of a non-empty list of `points`."""
    return (_mean(p[0] for p in points), _mean(p[1] for p in points))


def _mean(values):
    values = list(values)
    return sum(values) / len(values)
