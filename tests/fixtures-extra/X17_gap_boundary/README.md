# X17_gap_boundary

Session gap boundary (S1-S3, rules.md: a new session starts when the gap is *over* 1,800,000 ms)
and the session's day (S5: the UTC date of its start).

- u1 e1 10:00:00.000, e2 10:30:00.000: gap exactly 1,800,000 ms, so one session.
- e3, e4, e5 at 11:00:00.001 (F3): 1,800,001 ms after e2, so a new session; the three equal
  timestamps share it.
- e6 at 23:59:59, e7 (a purchase of 300) at 00:10:00 the next day: one session that started on
  Oct 3, so it is Oct 3's: no Oct 4 row.
- u2 e8 10:00:00.000 and e9 10:30:00.000, both F3: exactly 1,800,000 ms, one session.

u1: 3 sessions, 7 events, 300 cents, shoes. u2: 1 session, 2 events. organic: 4 sessions, 1 purchasing.
