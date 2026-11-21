# X13_top_category

top_category (O3, E20, O5). Products: p1 shoes, p2 books, p3 `Toys` (upper case); p9 and
`P1` are not in products.csv, so `unknown`.

- ua Oct 3: session 09:00 buys p1 100 three times (shoes 300), session 14:00 buys p2 400
  → pooled over the day: books 400 > shoes 300 → `books` (by count it would be shoes).
- ub Oct 3: p9 500 + p1 500 → tie, `shoes` < `unknown`. Oct 4: p9 500 + p2 300 → `unknown`.
- uc: Oct 3 23:50 p1 300 + Oct 4 00:10 p2 500 (one session, day Oct 3) → `books`, 800;
  Oct 4 10:00 p1 400 → `shoes`, 400.
- ud: Oct 3 view only → null. Oct 4 view + a late purchase (dropped) → null, 0, and not a
  purchasing session.
- ue: p2 priced `19.99` (dropped) + p1 100 → `shoes`, 1 event.
- uf: `P1` 700 + p2 600 → `unknown` (700).
- ug: p3 500 + p2 500 → tie, `Toys` < `books` byte-wise (upper case first).
- uh: session 09:00 p2 250, session 12:00 p1 250 → tie across sessions → `books`.

All organic. Oct 3: 10 sessions (ua 2, uh 2), 9 purchasing (not ud), 5,400. Oct 4: 3
sessions, 2 purchasing, 1,200.
