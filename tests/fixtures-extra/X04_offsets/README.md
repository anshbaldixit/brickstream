# X04_offsets

F2 offsets (T2, T10, E19). UTC = local − offset, and the sign covers the minutes.

| User | ts | UTC | Effect |
|---|---|---|---|
| ua | `2026-10-02T22:00:00-05:00` | Oct 3 03:00 | day Oct 3 |
| ub | `2026-10-03T21:00:00-03:30` | Oct 4 00:30 | day Oct 4 (−2:30 would give Oct 3) |
| uc | `2026-10-01T03:00:00+05:00` | Sep 30 22:00 | before the window: dropped |
| ud | `2026-09-30T21:00:00-05:00` | Oct 1 02:00 | inside: day Oct 1 |
| ue | `2026-10-14T23:30:00-00:30` | Oct 15 00:00 | at window_end: dropped |
| uf | `2026-10-15T05:29:59+05:30` | Oct 14 23:59:59 | day Oct 14 |
| ug | `2026-10-03T05:44:59+05:45` | Oct 2 23:59:59 | day Oct 2 |
| uh | `2026-10-03T14:30:00-09:30` | Oct 4 00:00 | day Oct 4 |
| ui | `2026-10-04T11:59:59+12:00` | Oct 3 23:59:59 | day Oct 3 |

Clicks in F2: uk's session is Oct 3 10:00Z. `k1` c_1 `05:30-04:00` = 09:30Z credits.
`k2` c_2 `09:30-01:00` = 10:30Z is after the start, so it neither credits nor assists
(with the sign flipped it would be 08:30Z and win). ul's session is Oct 1 00:30Z; `k3`
c_2 `2026-09-30T20:00:00-04:00` = Oct 1 00:00Z credits it.
