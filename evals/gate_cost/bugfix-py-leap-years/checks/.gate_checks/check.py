from calendar_utils import days_in_month, is_leap

for year, leap in [(1900, False), (2000, True), (2024, True), (2023, False), (2100, False), (1600, True)]:
    assert is_leap(year) is leap, year
assert days_in_month(1900, 2) == 28
assert days_in_month(2000, 2) == 29
assert days_in_month(2023, 4) == 30
assert days_in_month(2023, 12) == 31
