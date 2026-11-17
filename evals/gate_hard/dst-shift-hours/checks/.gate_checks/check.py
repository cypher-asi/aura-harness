from shifts import shift_hours

NY, LDN = "America/New_York", "Europe/London"
cases = [
    ("2026-03-08 00:00", "2026-03-08 06:00", NY, 5.0),
    ("2026-11-01 00:00", "2026-11-01 06:00", NY, 7.0),
    ("2026-11-01 01:30", "2026-11-01 03:00", NY, 2.5),
    ("2026-06-01 09:00", "2026-06-01 17:00", NY, 8.0),
    ("2026-06-01 22:00", "2026-06-02 06:00", NY, 8.0),
    ("2026-03-29 00:00", "2026-03-29 03:00", LDN, 2.0),
    ("2026-10-25 00:00", "2026-10-25 03:00", LDN, 4.0),
]
for start, end, tz, want in cases:
    got = shift_hours(start, end, tz)
    assert abs(got - want) < 1e-9, (start, end, tz, got, want)
for start, end, tz in [
    ("2026-03-08 02:30", "2026-03-08 06:00", NY),
    ("2026-03-08 00:00", "2026-03-08 02:15", NY),
    ("2026-03-29 01:30", "2026-03-29 05:00", LDN),
]:
    try:
        shift_hours(start, end, tz)
    except ValueError:
        pass
    else:
        raise AssertionError(f"{start}..{end} in {tz} should raise ValueError")
print("ok")
