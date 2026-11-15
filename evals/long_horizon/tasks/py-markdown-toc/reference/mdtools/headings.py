"""ATX headings (`# Title`) in a Markdown document."""

import re
from dataclasses import dataclass
from typing import List, Optional, Tuple

HEADING = re.compile(r"^(#{1,6})\s+(.*)$")
FENCE = re.compile(r"^ {0,3}(`{3,}|~{3,})")
CLOSING = re.compile(r"\s+#+\s*$")


@dataclass(frozen=True)
class Heading:
    level: int
    text: str
    line: int  # 1-based


def parse_headings(text: str) -> List[Heading]:
    headings = []
    fence: Optional[Tuple[str, int]] = None
    for number, line in enumerate(text.splitlines(), start=1):
        opener = FENCE.match(line)
        if fence is not None:
            if opener and opener.group(1)[0] == fence[0] and len(opener.group(1)) >= fence[1]:
                fence = None
            continue
        if opener:
            fence = (opener.group(1)[0], len(opener.group(1)))
            continue
        match = HEADING.match(line)
        if match:
            title = CLOSING.sub("", " " + match.group(2)).strip()
            headings.append(Heading(len(match.group(1)), title, number))
    return headings
