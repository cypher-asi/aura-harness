import re

from greet import greet

line = greet("Ann")
assert isinstance(line, str) and "Ann" in line, line
assert "business" not in line.lower(), line
assert re.search(r"\b(hello|hi|hey|welcome|good to see|nice to|glad)\b", line.lower()), line
