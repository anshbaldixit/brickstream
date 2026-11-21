# Extra fixtures

16 cases in the kit's format (`input/` laid out like `/data`, `expected/` as CSV with `day` as `YYYY-MM-DD` and an empty `top_category` for null). Each one pins rules that the kit's 36 fixtures leave uncovered. Every expected table was written by hand from `kit/rules.md`, and each case's `README.md` explains its rows. Rule IDs refer to `docs/spec-checklist.md`.

```
.venv/Scripts/python.exe kit/starter-duckdb/starter-duckdb/scripts/run_fixtures.py tests/fixtures-extra -- <cmd>
```
On Windows, write a venv Python in `<cmd>` as `'.venv\Scripts\python.exe'`; the forward-slash form fails in CreateProcess.

| Case | Pins | Generator makes it? |
|---|---|---|
| X01_quoted_empty | `""` = null in every column: empty user, event_id, product, type, ts, purchase price; link sides; click fields | yes |
| X02_quoted_all_columns | quotes in every column incl. ts (F1/F2/F3), ingest_ts, price; quoted vs unquoted event_id is one event; click copies quoted differently | yes |
| X03_malformed_lines | quote placement (space next to a quote, inner/lone/escaped quotes, comma inside quotes), 8 fields, empty lines, in events, clicks and links | partly |
| X04_offsets | negative and half-hour offsets (`-03:30` adds 3 h 30 m), offsets moving the date and the window edge, F2 clicks | yes |
| X05_bad_calendar | Sep 31, hour 24, minute 60, second 60 must be rejected, not normalized into the window; every other bad form | mostly |
| X06_feb_nonleap_window | window read from the manifest (Feb 2027); Feb 29/30 in a non-leap year; click at exactly window_start − 7 d | yes |
| X07_feb_leap_window | Feb 29, 2028 is valid and is a session day; 7 days across Feb 29 | yes |
| X08_big_numbers | F3 with 30 leading zeros; F3 and prices that wrap into range at 2^32/2^64; price edges; a 2.2e9 revenue (needs 64 bits) | partly |
| X09_dedup_order | malformed earliest copy never kept; dedup before validation and before the late check; quoted IDs; across files; identical copies | yes |
| X10_click_id_bytewise | credit ties broken by byte-wise click_id: `k10` < `k9`, `K5` < `k1`, `k2` < `k20`; ties across F1/F2/F3 | yes |
| X11_invalid_click_no_shadow | a later click with an unknown campaign or bad ts doesn't hide an earlier valid one; credit across midnight; purchasing_sessions | yes |
| X12_session_drops | late/invalid/duplicate events never bridge a gap; ms-precision late check; read order; early ingest | yes |
| X13_top_category | pooled across the day's sessions; by sum, not count; ties with `unknown` and with an upper-case category | yes |
| X14_links | a 299-hop chain over 3 files; an upper-case link side splits a person; `\r` and a space in a link ID; a kiosk merging three people's sessions | yes (except `\r`, space) |
| X15_odd_window | a 2.5-day window starting at noon: days are UTC dates, not offsets from window_start | no (rules only) |
| X16_multi_file | several files per folder, non-`part-` names, header-only files, dedup and links across files | yes |

## How they were checked
- `docs/tools/oracle.py`, an independent pure-Python implementation, passes all 36 kit fixtures, these 16, and the dev set (exact MATCH with `kit/dev-answer`).
- `docs/tools/mutation_check.py` plants 24 typical bugs in copies of the oracle (quoted-empty kept as text, offset minute sign, lenient calendar, 64-bit wrap, hard-coded window, numeric click_id order, late filter before dedup, and so on). Every one makes at least one fixture fail, and 20 of the 24 pass all 36 kit fixtures, so only these cases catch them.

## Maintenance
Inputs and expected tables live in `build_fixtures.py`, which writes them byte-exact (`.gitattributes` here turns off line-ending conversion, since X14 has a `\r`). To change a case, edit the script, rerun it, and update the hand-written expected rows and the README text together.
