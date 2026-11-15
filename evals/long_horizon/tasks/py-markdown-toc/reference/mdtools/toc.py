"""Tables of contents."""

from .headings import parse_headings
from .slug import SlugRegistry

START = "<!-- toc -->"
STOP = "<!-- tocstop -->"


def build_toc(text: str, min_level: int = 2, max_level: int = 3) -> str:
    registry = SlugRegistry()
    lines = []
    for heading in parse_headings(text):
        slug = registry.unique(heading.text)
        if min_level <= heading.level <= max_level:
            indent = "  " * (heading.level - min_level)
            lines.append(f"{indent}- [{heading.text}](#{slug})")
    return "\n".join(lines)


def insert_toc(text: str, min_level: int = 2, max_level: int = 3) -> str:
    lines = text.splitlines(keepends=True)
    stripped = [line.strip() for line in lines]
    if START not in stripped or STOP not in stripped:
        raise ValueError("missing <!-- toc --> or <!-- tocstop --> marker")
    start = stripped.index(START)
    stop = stripped.index(STOP, start)
    toc = build_toc(text, min_level, max_level)
    body = [toc + "\n"] if toc else []
    return "".join(lines[: start + 1] + body + lines[stop:])
