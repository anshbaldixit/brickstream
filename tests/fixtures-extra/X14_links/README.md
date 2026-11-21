# X14_links

Linked IDs (L3, L5, L7, L8, L9, C2, C9).

- Chain: `c000`…`c299` linked one after the next (alternating direction), the 299
  lines shuffled over 3 files. Events by `c299` (10:00) and `c150` (10:20) → one
  person, named `c000`, 1 session. Naive label propagation needs 299 rounds.
- Upper-case split: `ua1,ua2` and `UA2,ua3`. Events by `ua2` and `ua3` → two persons,
  `ua1` and `UA2` (`UA2` ≠ `ua2`).
- `\r`: the line `ub1,ub2\r` links `ub1` to `ub2\r`, not `ub2`. Events by ub1 and ub2 →
  two persons, `ub1` and `ub2`.
- Space: ` uc1,uc2` links ` uc1` (leading space) to `uc2`. Events by uc1 and uc2 → `uc1`
  alone, and `uc2`'s person is named ` uc1` (a space sorts first).
- Kiosk: `pa1,pa2`, `pa2,kz`, `kz,pb1`, `pc1,kz` → one person, named `kz`. Events pa1
  10:00, pb1 10:20, pa2 10:45, pc1 11:10 → one session of 4 (gaps under 30 min, across
  three people); pc1 12:00 → a second session. Clicks: `kk1` by pb1 c_1 09:30 credits
  session 1 and qualifies for session 2; `kk2` by pa2 c_2 11:30 credits session 2. So
  c_1 assists both, c_2 assists session 2.
