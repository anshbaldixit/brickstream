# X10_click_id_bytewise

Credit ties on `ts` go to the byte-wise smallest click_id (K5, T13). Each user has one
10:00 session on their own day, and two clicks for c_1 and c_2 at the same instant.
Both campaigns assist every session.

| Day | User | Clicks at 09:00 | Credited | Why |
|---|---|---|---|---|
| Oct 3 | ua | `k9` c_1, `k10` c_2 | c_2 | `k10` < `k9` byte-wise (numeric order says k9) |
| Oct 4 | ub | `k1` c_2, `K5` c_1 | c_1 | upper case sorts first |
| Oct 5 | uc | `k20` c_2, `k2` c_1 | c_1 | a prefix sorts first |
| Oct 6 | ud | `k4` c_2 (F1), `k31` c_1 (F3 1791277200000, the same instant) | c_1 | a tie across forms; `k31` < `k4` |
| Oct 7 | ue | `k5` c_2 at 09:00:00.000, `k6` c_1 at 09:00:00.001 (F3) | c_1 | 1 ms later wins, no tie |
| Oct 8 | uf | `k70` c_1 (F1 09:00Z), `k7` c_2 (`11:00+02:00`, the same instant) | c_2 | a tie across forms; `k7` < `k70` |
