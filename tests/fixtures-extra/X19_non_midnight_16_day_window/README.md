# X19_non_midnight_16_day_window

A 16-day window that starts at 05:30 (window 2026-10-01T05:30:00Z to 2026-10-17T05:30:00Z): 17
calendar days are touched. Window edges (T9, T10), days are UTC dates (S5), and the click
edge: exactly window_start - 7 d is in the click window.

- u1 `e1` at 05:29:59 (before the window): dropped. `e2` at 05:30:00 (the first instant): kept,
  Oct 1. Click `k1` (c_1) at Sep 24 05:30:00 is exactly 7 d before the session start: credits.
- u3 `e3` 23:59:59 and `e4` 00:00:00 next day: one session, Oct 1, 2 events.
- u2 `e5` at Oct 17 05:29:59: kept, day Oct 17. `e6` at exactly the window end: dropped.
