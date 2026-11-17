"""Fit text into table columns."""
import unicodedata

ELLIPSIS = "…"


def _columns(ch):
    if unicodedata.combining(ch):
        return 0
    return 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1


def display_width(text):
    return sum(_columns(ch) for ch in text)


def truncate(text, width):
    """Shorten text to fit in `width` terminal columns, ending with "…" when cut."""
    if display_width(text) <= width:
        return text
    kept, used = [], 0
    for ch in text:
        cols = _columns(ch)
        if cols and used + cols > width - 1:
            break
        kept.append(ch)
        used += cols
    return "".join(kept) + ELLIPSIS
