# X09_dedup_order

Dedup on event_id (E3-E7). Later copies are listed first, so "first line wins" fails.

| Event | Copies (ingest) | Kept copy | Effect |
|---|---|---|---|
| e1 | 10:01 with 8 fields (malformed), 10:05 valid | 10:05 | `ua` row (E6: a malformed copy is never kept) |
| e2 | 10:01 with `"ub` (bad quote), 10:05 valid | 10:05 | `ub` row (E6) |
| e3 | `"e3"` uc 10:01, `e3` ud 10:05 | uc | `uc` row, no `ud` (quotes aren't part of the ID) |
| e4 | uf 10:05 in part-00000, ue 10:01 in part-00001 | ue | `ue` row, no `uf` (dedup across files) |
| e5 | ts Oct 3 10:00 ingest Oct 4 10:00:01 (late), ts Oct 4 09:00 ingest Oct 4 10:00:02 | the late one | no `ug` row (E7: dedup before the late check) |
| e6 | `Purchase` 10:01, `purchase` 10:02 | `Purchase` | no `uh` row (E5) |
| e7 | user `""` 10:01, `ui` 10:02 | empty user | no `ui` row (E5) |
| e8 | two identical copies, one fully quoted | either | `uj` 1 event (E4) |
| E9 / e9 | different IDs (case) | both | `uk` 2 events |
| e10 | un 10:03, un 10:03 (tied, identical), um 10:01 | um | `um` row, no `un` |
