"""Shift lengths for staff rotas."""
from datetime import datetime, timezone
from zoneinfo import ZoneInfo

FMT = "%Y-%m-%d %H:%M"


def _instant(text, zone):
    local = datetime.strptime(text, FMT).replace(tzinfo=zone, fold=0)
    utc = local.astimezone(timezone.utc)
    if utc.astimezone(zone).replace(tzinfo=None) != local.replace(tzinfo=None):
        raise ValueError(f"{text} does not exist in {zone.key}")
    return utc


def shift_hours(start, end, tz):
    """Hours worked between two local wall-clock times ("YYYY-MM-DD HH:MM") in time zone tz (an IANA name).

    A time that happens twice means its first occurrence; a time that doesn't exist raises ValueError.
    """
    zone = ZoneInfo(tz)
    return (_instant(end, zone) - _instant(start, zone)).total_seconds() / 3600
