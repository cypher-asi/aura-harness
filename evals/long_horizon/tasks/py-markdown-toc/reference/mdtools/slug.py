"""GitHub-style heading anchors."""

from typing import Dict


def slugify(text: str) -> str:
    kept = "".join(c for c in text.strip().lower() if c.isalnum() or c in " -_")
    return kept.replace(" ", "-")


class SlugRegistry:
    def __init__(self) -> None:
        self._seen: Dict[str, int] = {}

    def unique(self, text: str) -> str:
        slug = slugify(text)
        count = self._seen.get(slug, 0)
        self._seen[slug] = count + 1
        return slug if count == 0 else f"{slug}-{count}"
