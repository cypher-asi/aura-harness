from intervals import merge

assert merge([]) == []
assert merge([[5, 6], [1, 2], [3, 4]]) == [[1, 6]]
assert merge([[5, 6], [1, 2], [3, 3]]) == [[1, 3], [5, 6]]
assert merge([[1, 4], [2, 3]]) == [[1, 4]]
assert merge([[1, 2], [4, 5]]) == [[1, 2], [4, 5]]
assert merge([[-3, -1], [0, 0], [10, 12], [11, 20]]) == [[-3, 0], [10, 20]]
data = [[3, 4], [1, 2]]
merge(data)
assert data == [[3, 4], [1, 2]], "input modified"
try:
    merge([[2, 1]])
except ValueError:
    pass
else:
    raise AssertionError("start > end must raise ValueError")
