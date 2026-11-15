"""Shift lengths for staff rotas."""
from datetime import datetime


def shift_hours(start, end, tz):
    """Hours worked between two local wall-clock times ("YYYY-MM-DD HH:MM") in time zone tz (an IANA name)."""
    fmt = "%Y-%m-%d %H:%M"
    a = datetime.strptime(start, fmt)
    b = datetime.strptime(end, fmt)
    return (b - a).total_seconds() / 3600
