# X15_odd_window

The rules don't promise a midnight start or a 14-day window (IN7). This one runs from
2026-10-01T12:00:00Z to 2026-10-04T00:00:00Z. The generator never does this, but the
manifest allows it, and a session's day is its UTC date whatever the window.

- ua Oct 1 11:59:59: before the window, dropped. ub Oct 1 12:00:00: kept, day Oct 1;
  `k1` c_1 at exactly window_start − 7 d (Sep 24 12:00) credits it.
- uc Oct 2 00:30 → day Oct 2 (a day index counted from window_start would say Oct 1).
- ud Oct 3 23:59:59 → kept, day Oct 3. ue Oct 4 00:00 → at window_end, dropped.
- uf Oct 1 23:50 + Oct 2 00:10 → one session, day Oct 1.
