# X08_big_numbers

Long digit strings and 64-bit sums (T4, T5, E12, O6).

F3: `ua` has 30 leading zeros before 1791021600000 → valid, Oct 3 10:00. Dropped:
`ub` 18446745864731151616 (= 2^64 + Oct 3 10:00: wrapping u64/i64 arithmetic lands in
the window), `uc` 2·2^64 + the same, `ud` 2^63 + the same, `ue` 38 digits.

Prices (purchases of p1, Oct 3 10:00, one user each). Kept: `pa` 10000000, `pc` 1,
`pi` 25 zeros + 500, `pp` 10,000,000 with leading zeros. Dropped: 10000001, 0, 00,
+500, 1e3, 250000000, 2^64+1000 and 2^32+1000 (they wrap to 1000), ` 500`, `500 `, -0.

`whale`: 220 purchases of p2 at 10,000,000 each, one a minute from Oct 5 10:00 → one
session, revenue 2,200,000,000 (over 2^31), credited to c_1 by `k1` at 09:00.

Organic Oct 3: pa, pc, pi, pp, ua = 5 sessions, 4 purchasing, 20,000,501.
