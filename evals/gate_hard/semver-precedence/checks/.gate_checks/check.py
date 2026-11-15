import random
from semver import compare, sort_versions

spec = ["1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta",
        "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0"]
for seed in range(5):
    shuffled = spec[:]
    random.Random(seed).shuffle(shuffled)
    assert sort_versions(shuffled) == spec, sort_versions(shuffled)
cases = [
    ("1.0.0+build.1", "1.0.0", 0),
    ("1.0.0+build-1", "1.0.0", 0),
    ("1.0.0-alpha+001", "1.0.0-alpha", 0),
    ("1.0.0+20130313144700", "1.0.0-beta+exp.sha.5114f85", 1),
    ("2.0.0", "10.0.0", -1),
    ("1.0.0-2", "1.0.0-10", -1),
    ("1.0.0-a", "1.0.0-1", 1),
    ("1.0.0-alpha.1", "1.0.0-alpha.1.0", -1),
    ("1.0.0-0", "1.0.0-0A", -1),
    ("1.0.0-alpha-1", "1.0.0-alpha", 1),
    ("1.2.3", "1.2.3", 0),
]
for a, b, want in cases:
    assert compare(a, b) == want, (a, b, compare(a, b), want)
    assert compare(b, a) == -want, (b, a)
print("ok")
