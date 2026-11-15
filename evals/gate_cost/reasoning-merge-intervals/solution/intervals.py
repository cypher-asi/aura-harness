def merge(intervals):
    """Merge inclusive integer intervals.

    `intervals` is a list of [start, end] pairs in any order. Intervals
    that overlap or are adjacent ([1, 2] and [3, 5] cover every integer
    from 1 to 5) merge into one. Returns a new list of [start, end]
    pairs sorted by start; the input is not modified. A pair with
    start > end raises ValueError.
    """
    merged = []
    for start, end in sorted(intervals):
        if start > end:
            raise ValueError(f"bad interval [{start}, {end}]")
        if merged and start <= merged[-1][1] + 1:
            merged[-1][1] = max(merged[-1][1], end)
        else:
            merged.append([start, end])
    return merged
