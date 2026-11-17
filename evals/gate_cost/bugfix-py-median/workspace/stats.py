def median(values):
    """Return the median of a non-empty list of numbers."""
    ordered = sorted(values)
    return ordered[len(ordered) // 2]
