# X18_credit_tie_and_7d_edges

Credit tie, the 7-day edges and assist-only campaign days (K5-K9, A1-A4). u1 has one view at
Oct 10 12:00:00.000.

| Click | Campaign | ts | Effect |
|---|---|---|---|
| `k_10` | c_1 | Oct 3 12:00:00.000 (exactly 7 d before) | qualifies; ties with `k_9` on ts |
| `k_9` | c_2 | same ts | qualifies; `k_10` < `k_9` byte-wise, so c_1 is credited |
| `k_old` | c_3 | Oct 3 11:59:59.999 (F3: 7 d + 1 ms before) | too old: neither credits nor assists |
| `k_late` | c_3 | Oct 10 12:00:00.001 (F3, 1 ms after the start) | later than the session start: neither |

c_1 is credited and assists; c_2 only assists, so its campaign-day row has 0 sessions and 1
assisted session. c_3 has no row.
