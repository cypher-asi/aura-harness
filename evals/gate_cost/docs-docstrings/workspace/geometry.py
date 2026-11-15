import math


def circle_area(radius):
    return math.pi * radius ** 2


def rect_area(width, height):
    return width * height


def distance(a, b):
    return math.hypot(b[0] - a[0], b[1] - a[1])


def centroid(points):
    return (_mean(p[0] for p in points), _mean(p[1] for p in points))


def _mean(values):
    values = list(values)
    return sum(values) / len(values)
