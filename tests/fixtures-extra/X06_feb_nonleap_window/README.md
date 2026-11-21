# X06_feb_nonleap_window

A window from the manifest that isn't the kit's October one: 2027-02-22 to
2027-03-08, in a non-leap year (IN6, T8, T10, T11).

- ua `2027-02-29`, ub `2027-02-30`: bad dates (a lenient parser makes Mar 1, Mar 2): dropped.
- uc: Feb 28 23:50 view + Mar 1 00:10 purchase p1 1000 → one session, day Feb 28.
  Click `k3` c_2 at Feb 28 23:00 credits it.
- ud: `2027-03-01T01:00:00+02:00` = Feb 28 23:00Z → day Feb 28.
- ue: F3 `1803895200000` = 2027-03-01T10:00Z → day Mar 1.
- uf: Feb 21 23:59:59 (before the window, dropped) and Feb 22 00:00:00 (kept: day Feb 22,
  not Feb 21). Click `k1` c_1 at exactly window_start − 7 d (Feb 15 00:00) is valid
  and exactly 7 d before: credits.
- ug at window_end (Mar 8 00:00): dropped. uh at Mar 7 23:59:59: kept.
- ui at 2026-10-03 (the kit's window, not this one): dropped.
