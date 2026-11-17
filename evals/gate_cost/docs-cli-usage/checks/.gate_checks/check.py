import re

text = open("README.md").read()
assert "## Usage" in text
usage = text.split("## Usage", 1)[1]
for flag in ["--name", "--shout", "--times"]:
    assert flag in usage, flag
assert re.search(r"python3? cli\.py", usage), "no example invocation"
