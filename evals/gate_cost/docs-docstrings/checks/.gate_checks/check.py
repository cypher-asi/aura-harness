import ast
import math

import geometry

tree = ast.parse(open("geometry.py").read())
public = [n for n in tree.body if isinstance(n, ast.FunctionDef) and not n.name.startswith("_")]
assert {f.name for f in public} == {"circle_area", "rect_area", "distance", "centroid"}
for f in public:
    doc = ast.get_docstring(f) or ""
    assert len(doc) >= 20, f.name
assert math.isclose(geometry.circle_area(2), 4 * math.pi)
assert geometry.rect_area(3, 4) == 12
assert geometry.distance((0, 0), (3, 4)) == 5
assert geometry.centroid([(0, 0), (2, 0), (1, 3)]) == (1, 1)
