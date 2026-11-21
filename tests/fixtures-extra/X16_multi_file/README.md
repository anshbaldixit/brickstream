# X16_multi_file

Several files per folder, with names other than `part-NNNNN.csv` and header-only files
(IN1, IN2). Every `*.csv` counts.

- events: `part-00000.csv` (u1 10:00, u2 10:50, `e5` by u3 ingested 10:05),
  `part-00003.csv` (u1 10:20 purchase p1 500), `other.csv` (u1 10:40, `e5` by u4
  ingested 10:01), `part-00010.csv` (header only).
- links: `part-00000.csv` (header only), `part-00001.csv` (`u2,u1`), `b.csv` (`u5,u6`).
- clicks: `part-00000.csv` (`k1` by u2 c_1 09:00), `part-00001.csv` (header only),
  `extra.csv` (`k2` by u1 c_2 09:30, `k3` by u4 c_1 09:59:59).

Person u1 (u1 + u2): one session 10:00-10:50, 4 events, 500, shoes; credited c_2 (09:30),
assisted by c_1 and c_2. `e5` is u4's (earliest ingest, other file): credited c_1.
