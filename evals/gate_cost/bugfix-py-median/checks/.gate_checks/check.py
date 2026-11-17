from stats import median

assert median([3, 1, 2]) == 2
assert median([1, 2, 3, 4]) == 2.5
assert median([5]) == 5
assert median([8, 1, 1, 2]) == 1.5
data = [4, 2, 9, 1]
median(data)
assert data == [4, 2, 9, 1], "median must not reorder its input"
