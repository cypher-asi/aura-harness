"""ATX headings (`# Title`) in a Markdown document."""

import re
from dataclasses import dataclass
from typing import List

HEADING = re.compile(r"^(#{1,6})\s+(.*)$")


@dataclass(frozen=True)
class Heading:
    level: int
    text: str
    line: int  # 1-based


def parse_headings(text: str) -> List[Heading]:
    headings = []
    for number, line in enumerate(text.splitlines(), start=1):
        match = HEADING.match(line)
        if match:
            headings.append(Heading(len(match.group(1)), match.group(2).strip(), number))
    return headings
