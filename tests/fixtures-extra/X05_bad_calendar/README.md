# X05_bad_calendar

Bad timestamps (T3, T7, T8), one user each, all dropped. The calendar errors are the
kind a lenient parser would *normalize* into the window: `2026-09-31` → Oct 1,
`T24:00:00` → next day, `10:60:00` → 11:00, `10:00:60` → 10:01. Each of those rows'
ingest_ts sits just after the normalized instant, so the late rule can't hide the bug.
Also: fractional seconds, lowercase `z`/`t`, no zone, space for `T`, `1.791021600e12`,
`-5`, `+N`, leading/trailing spaces on F3, offsets `-24:00`, `+5:30`, `+05`,
`+05:30Z`, ` +05:00`, basic ISO, one-digit day, no seconds, letters in digit positions.

Survivors: `ok1` (`2026-10-03T10:00:00+00:00`, valid) and `ok2` (Oct 3 23:59:59Z).
ok1's clicks `k1` (c_1, `09:00:60Z`) and `k2` (c_2, `2026-09-31T10:00:00Z`) are bad
timestamps too, so ok1 is organic.
