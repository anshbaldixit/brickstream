# X02_quoted_all_columns

Quotes are CSV syntax in every column (C5), including `ts` in all three forms,
`ingest_ts` (T12), `price_cents`, `click_id`, `campaign_id` and link IDs (L11).

- `u2,u1` linked (quoted). Person `u1`: e1 10:00 purchase p1 1000, e2 10:05 (F3, quoted),
  e3 10:10 (F2 `12:10+02:00`, quoted) → one session, 3 events, 1000, shoes.
- `"e4"` (u3, ingest 10:01) and `e4` (u4, ingest 10:05, listed first) are one event (E3):
  u3's copy wins; price `"0250"` = 250, books.
- Click `k1` appears 3 times quoted differently (K3): credits u1 once. `k2` (quoted F3
  09:30) credits u3.
