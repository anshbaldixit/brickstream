# X12_session_drops

What reaches sessionizing (E14-E19).

- ua: 10:00, 10:25 (ingested 24 h + 1 s later: late), 10:50 → the late event doesn't
  bridge the gap: 2 sessions, 2 events.
- ub: 10:00, 10:25 typed `View` (invalid), 10:50 → 2 sessions, 2 events.
- uc: `c1` at 10:00 (ingest 10:01) plus a later copy of `c1` with ts 10:25 (ingest 10:30),
  then `c2` 10:50 → only the kept copy counts: 2 sessions, 2 events.
- ud: F3 10:00:00.500, ingest Oct 4 10:00:00 → 24 h − 500 ms: kept.
- ue: F3 10:00:00.500, ingest Oct 4 10:00:01 → 24 h + 500 ms: late, dropped (rounding ts
  to seconds would keep it).
- uf: F3 Oct 3 23:59:59.999 and Oct 4 00:20 → one session, day Oct 3.
- ug: events listed newest first across two files: 12:00, 11:00, 10:40 | 10:20, 10:00 →
  sessions 10:00-11:00 (4 events) and 12:00 (1).
- uh: ingested 5 h before ts: kept.
