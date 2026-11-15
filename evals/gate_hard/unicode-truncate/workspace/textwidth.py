"""Fit text into table columns."""


def truncate(text, width):
    """Shorten text to fit in `width` terminal columns, ending with "…" when cut."""
    if len(text) <= width:
        return text
    return text[: width - 1] + "…"
