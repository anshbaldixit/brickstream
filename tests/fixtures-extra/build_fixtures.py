#!/usr/bin/env python3
"""Writes tests/fixtures-extra/X*/ (input/, expected/, README.md) in the kit's fixture format.

The inputs are generated here so they're reviewable and reproducible. Every EXPECTED table
below was worked out by hand from kit/rules.md, not produced by any implementation; each
case's README says why each row is what it is. Rule IDs (C6, T8, ...) refer to
docs/spec-checklist.md.

Run from the repo root:  .venv/Scripts/python.exe tests/fixtures-extra/build_fixtures.py
"""
import datetime
import os
import random
import shutil

HERE = os.path.dirname(os.path.abspath(__file__))

EV = "event_id,user_id,ts,ingest_ts,event_type,product_id,price_cents"
CK = "click_id,user_id,campaign_id,ts"
LK = "user_id_a,user_id_b"
UD = "user_id,day,sessions,events,revenue_cents,top_category"
CD = "campaign_id,channel,day,sessions,purchasing_sessions,revenue_cents,assisted_sessions"
OCT = ("2026-10-01T00:00:00Z", "2026-10-15T00:00:00Z")
PRODUCTS = ["p1,shoes", "p2,books"]
CAMPAIGNS = ["c_1,search", "c_2,social"]
T = "2026-10-03T10:00:00Z"   # the usual event time
I = "2026-10-03T10:01:00Z"   # and its ingest time


def at(stamp, minutes=0):
    """F1 string `minutes` after the F1 string `stamp` (input generation only)."""
    t = datetime.datetime.strptime(stamp, "%Y-%m-%dT%H:%M:%SZ") + datetime.timedelta(minutes=minutes)
    return t.strftime("%Y-%m-%dT%H:%M:%SZ")


def ev(eid, user, ts, kind="view", product="p1", price="", ingest=None):
    """One event line; ingest defaults to ts + 1 minute (ts must then be F1)."""
    return f"{eid},{user},{ts},{ingest or at(ts, 1)},{kind},{product},{price}"


CASES = []


def case(name, readme, events, clicks=(), links=(), user_daily=(), campaign_daily=(),
         window=OCT, products=PRODUCTS, campaigns=CAMPAIGNS):
    CASES.append(dict(name=name, readme=readme, events=events, clicks=clicks, links=links,
                      user_daily=user_daily, campaign_daily=campaign_daily, window=window,
                      products=products, campaigns=campaigns))


# --------------------------------------------------------------------------- X01
case("X01_quoted_empty", """\
`""` is an empty field, i.e. null, in every column (C6). The kit never uses `""`; the
generator writes it for empty users, products, types, view prices and link sides.

| Row | Input | Effect |
|---|---|---|
| e1 | user `""` | dropped (E8) |
| e2 | event_id `""` | dropped (E2): no `ub` row |
| e3 | purchase priced `""` | dropped (E12): no `uc` row |
| e4 | view priced `""` | kept (E13): `ud` row |
| e5, e6 | product `""` / empty | dropped (E11): no `ue`, `uf` rows |
| e7 | event_type `""` | dropped (E10): no `ug` row |
| e8 | ts `""` | dropped (T7): no `uh` row |
| e9 | user empty, unquoted | dropped (E8) |
| e10 | cart priced `""` | kept: `ui` row |
| links | `"",uj`, `ul,""`, `"",""` | all dropped (L2), so `uj` and `ul` keep their own names instead of an empty one |
| clicks | `""` click_id (uj), `""` user, `""` campaign (um), `""` ts (um) | all dropped (K2, K4): `uj`, `um` are organic |
| click k5 | `ud`, c_2, quoted ts 09:30 | valid: credits `ud`'s 10:00 session |
""",
     events=[
         f'e1,"",{T},{I},view,p1,',
         f'"",ub,{T},{I},view,p1,',
         f'e3,uc,{T},{I},purchase,p1,""',
         f'e4,ud,{T},{I},view,p1,""',
         f'e5,ue,{T},{I},view,"",',
         f'e6,uf,{T},{I},view,,',
         f'e7,ug,{T},{I},"",p1,',
         f'e8,uh,"",{I},view,p1,',
         f'e9,,{T},{I},view,p1,',
         f'e10,ui,{T},{I},cart,p2,""',
         f'e11,uj,{T},{I},view,p1,',
         f'e12,ul,{T},{I},view,p1,',
         f'e13,um,{T},{I},view,p1,',
     ],
     clicks=[
         '"",uj,c_1,2026-10-03T09:00:00Z',
         'k2,"",c_1,2026-10-03T09:00:00Z',
         'k3,um,"",2026-10-03T09:00:00Z',
         'k4,um,c_2,""',
         'k5,ud,c_2,"2026-10-03T09:30:00Z"',
     ],
     links=['"",uj', 'ul,""', '"",""'],
     user_daily=[
         "ud,2026-10-03,1,1,0,",
         "ui,2026-10-03,1,1,0,",
         "uj,2026-10-03,1,1,0,",
         "ul,2026-10-03,1,1,0,",
         "um,2026-10-03,1,1,0,",
     ],
     campaign_daily=[
         "c_2,social,2026-10-03,1,0,0,1",
         "organic,organic,2026-10-03,4,0,0,0",
     ])

# --------------------------------------------------------------------------- X02
case("X02_quoted_all_columns", """\
Quotes are CSV syntax in every column (C5), including `ts` in all three forms,
`ingest_ts` (T12), `price_cents`, `click_id`, `campaign_id` and link IDs (L11).

- `u2,u1` linked (quoted). Person `u1`: e1 10:00 purchase p1 1000, e2 10:05 (F3, quoted),
  e3 10:10 (F2 `12:10+02:00`, quoted) → one session, 3 events, 1000, shoes.
- `"e4"` (u3, ingest 10:01) and `e4` (u4, ingest 10:05, listed first) are one event (E3):
  u3's copy wins; price `"0250"` = 250, books.
- Click `k1` appears 3 times quoted differently (K3): credits u1 once. `k2` (quoted F3
  09:30) credits u3.
""",
     events=[
         '"e1","u1","2026-10-03T10:00:00Z","2026-10-03T10:01:00Z","purchase","p1","1000"',
         '"e2","u2","1791021900000","2026-10-03T10:06:00Z","view","p2",""',
         '"e3",u2,"2026-10-03T12:10:00+02:00","2026-10-03T10:11:00Z","cart","p2","5"',
         'e4,u4,2026-10-03T10:00:00Z,2026-10-03T10:05:00Z,purchase,p2,250',
         '"e4","u3","2026-10-03T10:00:00Z","2026-10-03T10:01:00Z","purchase","p2","0250"',
     ],
     clicks=[
         '"k1","u1","c_1","2026-10-03T09:00:00Z"',
         'k1,u1,c_1,2026-10-03T09:00:00Z',
         '"k1",u1,"c_1",2026-10-03T09:00:00Z',
         '"k2","u3","c_2","1791019800000"',
     ],
     links=['"u2","u1"'],
     user_daily=[
         "u1,2026-10-03,1,3,1000,shoes",
         "u3,2026-10-03,1,1,250,books",
     ],
     campaign_daily=[
         "c_1,search,2026-10-03,1,1,1000,1",
         "c_2,social,2026-10-03,1,1,250,1",
     ])

# --------------------------------------------------------------------------- X03
case("X03_malformed_lines", '''\
Malformed lines in events, clicks and links (C3, C4, C7, C8, E1, K1, L1). Every
malformed event line has its own user, so any leak shows up as an extra row.

Events, all dropped: ` "ua"` (space before the quote), `"ub" ` (space after),
`u"c` (inner quote), `"ud` and `ue"` (unbalanced), a lone `"` (uf), `"""` (ug),
`"p""1"` (uh; no escaped quotes), `"ui,x"` (a comma inside quotes is still a
separator), 8 fields from a trailing comma (uj), `,x` appended (uk), an empty line, a
`"` added at the start of the line (ul), `""""` (um). Only `ok1` (10:00) and `ok2`
(10:05) survive.

Clicks for ok1: `k0` c_2 at 08:00 is valid. Every later c_1 click is malformed (5
fields, 3 fields, unbalanced quotes, space before a quote, empty line, empty
click_id, `"ok1,c_1"`), so ok1 is credited to c_2, and only c_2 assists.

Links, all malformed: ` "ok2",aa1`, `ok2, "aa2"`, `"ok2" ,aa3`, `ok1,"aa4,x"`,
`"ok1,aa5"`. So ok1 and ok2 keep their own names (`aa…` would sort first).
''',
     events=[
         ev("e1", "ok1", T),
         ev("e2", ' "ua"', T),
         ev("e3", '"ub" ', T),
         ev("e4", 'u"c', T),
         ev("e5", '"ud', T),
         ev("e6", 'ue"', T),
         ev("e7", "uf", T, price='"'),
         ev("e8", "ug", T, price='"""'),
         ev("e9", "uh", T, product='"p""1"'),
         ev("e10", '"ui,x"', T),
         ev("e11", "uj", T) + ",",
         ev("e12", "uk", T) + ",x",
         "",
         '"' + ev("e13", "ul", T),
         ev("e14", "um", T, price='""""'),
         ev("e15", "ok2", "2026-10-03T10:05:00Z"),
     ],
     clicks=[
         "k0,ok1,c_2,2026-10-03T08:00:00Z",
         "k1,ok1,c_1,2026-10-03T09:00:00Z,x",
         "k2,ok1,c_1",
         '"k3,ok1,c_1,2026-10-03T09:10:00Z',
         'k4,ok1,c_1,2026-10-03T09:20:00Z"',
         'k5,ok1, "c_1",2026-10-03T09:30:00Z',
         "",
         ",ok1,c_1,2026-10-03T09:40:00Z",
         'k7,"ok1,c_1",2026-10-03T09:50:00Z',
     ],
     links=[' "ok2",aa1', 'ok2, "aa2"', '"ok2" ,aa3', 'ok1,"aa4,x"', '"ok1,aa5"'],
     user_daily=[
         "ok1,2026-10-03,1,1,0,",
         "ok2,2026-10-03,1,1,0,",
     ],
     campaign_daily=[
         "c_2,social,2026-10-03,1,0,0,1",
         "organic,organic,2026-10-03,1,0,0,0",
     ])

# --------------------------------------------------------------------------- X04
case("X04_offsets", """\
F2 offsets (T2, T10, E19). UTC = local − offset, and the sign covers the minutes.

| User | ts | UTC | Effect |
|---|---|---|---|
| ua | `2026-10-02T22:00:00-05:00` | Oct 3 03:00 | day Oct 3 |
| ub | `2026-10-03T21:00:00-03:30` | Oct 4 00:30 | day Oct 4 (−2:30 would give Oct 3) |
| uc | `2026-10-01T03:00:00+05:00` | Sep 30 22:00 | before the window: dropped |
| ud | `2026-09-30T21:00:00-05:00` | Oct 1 02:00 | inside: day Oct 1 |
| ue | `2026-10-14T23:30:00-00:30` | Oct 15 00:00 | at window_end: dropped |
| uf | `2026-10-15T05:29:59+05:30` | Oct 14 23:59:59 | day Oct 14 |
| ug | `2026-10-03T05:44:59+05:45` | Oct 2 23:59:59 | day Oct 2 |
| uh | `2026-10-03T14:30:00-09:30` | Oct 4 00:00 | day Oct 4 |
| ui | `2026-10-04T11:59:59+12:00` | Oct 3 23:59:59 | day Oct 3 |

Clicks in F2: uk's session is Oct 3 10:00Z. `k1` c_1 `05:30-04:00` = 09:30Z credits.
`k2` c_2 `09:30-01:00` = 10:30Z is after the start, so it neither credits nor assists
(with the sign flipped it would be 08:30Z and win). ul's session is Oct 1 00:30Z; `k3`
c_2 `2026-09-30T20:00:00-04:00` = Oct 1 00:00Z credits it.
""",
     events=[
         ev("e1", "ua", "2026-10-02T22:00:00-05:00", ingest="2026-10-03T03:01:00Z"),
         ev("e2", "ub", "2026-10-03T21:00:00-03:30", ingest="2026-10-04T00:31:00Z"),
         ev("e3", "uc", "2026-10-01T03:00:00+05:00", ingest="2026-09-30T22:01:00Z"),
         ev("e4", "ud", "2026-09-30T21:00:00-05:00", ingest="2026-10-01T02:01:00Z"),
         ev("e5", "ue", "2026-10-14T23:30:00-00:30", ingest="2026-10-15T00:01:00Z"),
         ev("e6", "uf", "2026-10-15T05:29:59+05:30", ingest="2026-10-15T00:00:59Z"),
         ev("e7", "ug", "2026-10-03T05:44:59+05:45", ingest="2026-10-03T00:00:59Z"),
         ev("e8", "uh", "2026-10-03T14:30:00-09:30", ingest="2026-10-04T00:01:00Z"),
         ev("e9", "ui", "2026-10-04T11:59:59+12:00", ingest="2026-10-04T00:00:59Z"),
         ev("e10", "uk", T),
         ev("e11", "ul", "2026-10-01T00:30:00Z"),
     ],
     clicks=[
         "k1,uk,c_1,2026-10-03T05:30:00-04:00",
         "k2,uk,c_2,2026-10-03T09:30:00-01:00",
         "k3,ul,c_2,2026-09-30T20:00:00-04:00",
     ],
     user_daily=[
         "ua,2026-10-03,1,1,0,",
         "ub,2026-10-04,1,1,0,",
         "ud,2026-10-01,1,1,0,",
         "uf,2026-10-14,1,1,0,",
         "ug,2026-10-02,1,1,0,",
         "uh,2026-10-04,1,1,0,",
         "ui,2026-10-03,1,1,0,",
         "uk,2026-10-03,1,1,0,",
         "ul,2026-10-01,1,1,0,",
     ],
     campaign_daily=[
         "c_1,search,2026-10-03,1,0,0,1",
         "c_2,social,2026-10-01,1,0,0,1",
         "organic,organic,2026-10-01,1,0,0,0",
         "organic,organic,2026-10-02,1,0,0,0",
         "organic,organic,2026-10-03,2,0,0,0",
         "organic,organic,2026-10-04,2,0,0,0",
         "organic,organic,2026-10-14,1,0,0,0",
     ])

# --------------------------------------------------------------------------- X05
X05_BAD = [
    # user, ts, ingest (set just after the instant a lenient parser would produce,
    # so the late rule can't hide a parser that normalizes)
    ("ua", "2026-09-31T10:00:00Z", "2026-10-01T10:01:00Z"),   # Sep 31 -> Oct 1
    ("ub", "2026-10-03T24:00:00Z", "2026-10-04T00:01:00Z"),   # hour 24 -> Oct 4
    ("uc", "2026-10-03T10:60:00Z", "2026-10-03T11:01:00Z"),   # minute 60
    ("ud", "2026-10-03T10:00:60Z", "2026-10-03T10:02:00Z"),   # second 60
    ("ue", "2026-10-00T10:00:00Z", "2026-10-01T10:01:00Z"),   # day 00
    ("uf", "2026-13-03T10:00:00Z", I),                         # month 13
    ("ug", "2026-10-03T10:00:00.000Z", I),                     # fractional seconds
    ("uh", "2026-10-03T10:00:00z", I),                         # lowercase z
    ("ui", "2026-10-03t10:00:00Z", I),                         # lowercase t
    ("uj", "2026-10-03T10:00:00", I),                          # no zone
    ("uk", "2026-10-03 10:00:00Z", I),                         # space for T
    ("ul", "1.791021600e12", I),                               # exponent (= 10:00 as a float)
    ("um", "-5", I),                                           # negative
    ("un", "+1791021600000", I),                               # explicit sign
    ("uo", " 1791021600000", I),                               # leading space
    ("up", "1791021600000 ", I),                               # trailing space
    ("uq", "2026-10-03T10:00:00-24:00", I),                    # offset hour 24
    ("ur", "2026-10-03T10:00:00+5:30", I),                     # one-digit offset hour
    ("us", "2026-10-03T10:00:00+05", I),                       # no offset minutes
    ("ut", "2026-10-03T10:00:00+05:30Z", I),                   # offset and Z
    ("uu", "2026-10-03T10:00:00 +05:00", I),                   # space before offset
    ("uv", "20261003T100000Z", I),                             # basic ISO form
    ("uw", "2026-10-3T10:00:00Z", I),                          # one-digit day
    ("ux", "2026-10-03T10:00Z", I),                            # no seconds
    ("uy", "2026-10-03T10:00:00+05:3a", I),                    # letter in offset
    ("uz", "2026-1O-03T10:00:00Z", I),                         # letter O in month
]
case("X05_bad_calendar", """\
Bad timestamps (T3, T7, T8), one user each, all dropped. The calendar errors are the
kind a lenient parser would *normalize* into the window: `2026-09-31` → Oct 1,
`T24:00:00` → next day, `10:60:00` → 11:00, `10:00:60` → 10:01. Each of those rows'
ingest_ts sits just after the normalized instant, so the late rule can't hide the bug.
Also: fractional seconds, lowercase `z`/`t`, no zone, space for `T`, `1.791021600e12`,
`-5`, `+N`, leading/trailing spaces on F3, offsets `-24:00`, `+5:30`, `+05`,
`+05:30Z`, ` +05:00`, basic ISO, one-digit day, no seconds, letters in digit positions.

Survivors: `ok1` (`2026-10-03T10:00:00+00:00`, valid) and `ok2` (Oct 3 23:59:59Z).
ok1's clicks `k1` (c_1, `09:00:60Z`) and `k2` (c_2, `2026-09-31T10:00:00Z`) are bad
timestamps too, so ok1 is organic.
""",
     events=[ev(f"e{n}", u, ts, ingest=ing) for n, (u, ts, ing) in enumerate(X05_BAD, 1)] + [
         ev("e90", "ok1", "2026-10-03T10:00:00+00:00", ingest=I),
         ev("e91", "ok2", "2026-10-03T23:59:59Z"),
     ],
     clicks=[
         "k1,ok1,c_1,2026-10-03T09:00:60Z",
         "k2,ok1,c_2,2026-09-31T10:00:00Z",
     ],
     user_daily=[
         "ok1,2026-10-03,1,1,0,",
         "ok2,2026-10-03,1,1,0,",
     ],
     campaign_daily=["organic,organic,2026-10-03,2,0,0,0"])

# --------------------------------------------------------------------------- X06
case("X06_feb_nonleap_window", """\
A window from the manifest that isn't the kit's October one: 2027-02-22 to
2027-03-08, in a non-leap year (IN6, T8, T10, T11).

- ua `2027-02-29`, ub `2027-02-30`: bad dates (a lenient parser makes Mar 1, Mar 2): dropped.
- uc: Feb 28 23:50 view + Mar 1 00:10 purchase p1 1000 → one session, day Feb 28.
  Click `k3` c_2 at Feb 28 23:00 credits it.
- ud: `2027-03-01T01:00:00+02:00` = Feb 28 23:00Z → day Feb 28.
- ue: F3 `1803895200000` = 2027-03-01T10:00Z → day Mar 1.
- uf: Feb 21 23:59:59 (before the window, dropped) and Feb 22 00:00:00 (kept: day Feb 22,
  not Feb 21). Click `k1` c_1 at exactly window_start − 7 d (Feb 15 00:00) is valid
  and exactly 7 d before: credits.
- ug at window_end (Mar 8 00:00): dropped. uh at Mar 7 23:59:59: kept.
- ui at 2026-10-03 (the kit's window, not this one): dropped.
""",
     window=("2027-02-22T00:00:00Z", "2027-03-08T00:00:00Z"),
     events=[
         ev("e1", "ua", "2027-02-29T10:00:00Z", ingest="2027-03-01T10:01:00Z"),
         ev("e2", "ub", "2027-02-30T10:00:00Z", ingest="2027-03-02T10:01:00Z"),
         ev("e3", "uc", "2027-02-28T23:50:00Z"),
         ev("e4", "uc", "2027-03-01T00:10:00Z", "purchase", "p1", "1000"),
         ev("e5", "ud", "2027-03-01T01:00:00+02:00", ingest="2027-02-28T23:01:00Z"),
         ev("e6", "ue", "1803895200000", ingest="2027-03-01T10:01:00Z"),
         ev("e7", "uf", "2027-02-21T23:59:59Z"),
         ev("e8", "uf", "2027-02-22T00:00:00Z"),
         ev("e9", "ug", "2027-03-08T00:00:00Z"),
         ev("e10", "uh", "2027-03-07T23:59:59Z"),
         ev("e11", "ui", T),
     ],
     clicks=[
         "k1,uf,c_1,2027-02-15T00:00:00Z",
         "k3,uc,c_2,2027-02-28T23:00:00Z",
     ],
     user_daily=[
         "uc,2027-02-28,1,2,1000,shoes",
         "ud,2027-02-28,1,1,0,",
         "ue,2027-03-01,1,1,0,",
         "uf,2027-02-22,1,1,0,",
         "uh,2027-03-07,1,1,0,",
     ],
     campaign_daily=[
         "c_1,search,2027-02-22,1,0,0,1",
         "c_2,social,2027-02-28,1,1,1000,1",
         "organic,organic,2027-02-28,1,0,0,0",
         "organic,organic,2027-03-01,1,0,0,0",
         "organic,organic,2027-03-07,1,0,0,0",
     ])

# --------------------------------------------------------------------------- X07
case("X07_feb_leap_window", """\
A window holding February 29 of a leap year: 2028-02-22 to 2028-03-07 (T9, E19, O7).

- ua: `2028-02-29T10:00:00Z` is valid → day 2028-02-29. Click `k1` c_1 at Feb 22 10:00 is
  exactly 7 days before (Feb has 29 days): credits.
- ub: F3 `1835431200000` = 2028-02-29T10:00Z, purchase p2 700 → day Feb 29. Click `k2`
  c_2 at Feb 22 09:59:59 is 7 d + 1 s before: doesn't qualify, so organic.
- uc: Feb 28 23:50 + Feb 29 00:10 → one session, day Feb 28.
- ud: Feb 29 23:50 + Mar 1 00:10 purchase p1 300 → one session, day Feb 29.
- ue: `2028-03-01T01:30:00+02:00` = Feb 29 23:30Z → day Feb 29.
- uf: `2028-02-30`: bad, dropped.
""",
     window=("2028-02-22T00:00:00Z", "2028-03-07T00:00:00Z"),
     events=[
         ev("e1", "ua", "2028-02-29T10:00:00Z"),
         ev("e2", "ub", "1835431200000", "purchase", "p2", "700", ingest="2028-02-29T10:01:00Z"),
         ev("e3", "uc", "2028-02-28T23:50:00Z"),
         ev("e4", "uc", "2028-02-29T00:10:00Z"),
         ev("e5", "ud", "2028-02-29T23:50:00Z"),
         ev("e6", "ud", "2028-03-01T00:10:00Z", "purchase", "p1", "300"),
         ev("e7", "ue", "2028-03-01T01:30:00+02:00", ingest="2028-02-29T23:31:00Z"),
         ev("e8", "uf", "2028-02-30T10:00:00Z", ingest="2028-03-01T10:01:00Z"),
     ],
     clicks=[
         "k1,ua,c_1,2028-02-22T10:00:00Z",
         "k2,ub,c_2,2028-02-22T09:59:59Z",
     ],
     user_daily=[
         "ua,2028-02-29,1,1,0,",
         "ub,2028-02-29,1,1,700,books",
         "uc,2028-02-28,1,2,0,",
         "ud,2028-02-29,1,2,300,shoes",
         "ue,2028-02-29,1,1,0,",
     ],
     campaign_daily=[
         "c_1,search,2028-02-29,1,0,0,1",
         "organic,organic,2028-02-28,1,0,0,0",
         "organic,organic,2028-02-29,3,2,1000,0",
     ])

# --------------------------------------------------------------------------- X08
X08_PRICES = [
    ("pa", "10000000", True), ("pb", "10000001", False), ("pc", "1", True),
    ("pd", "0", False), ("pe", "00", False), ("pf", "+500", False), ("pg", "1e3", False),
    ("ph", "250000000", False), ("pi", "0000000000000000000000000500", True),
    ("pj", "18446744073709552616", False),   # 2^64 + 1000: wraps to 1000 in u64
    ("pk", "4294968296", False),             # 2^32 + 1000: wraps to 1000 in u32
    ("pl", " 500", False), ("pm", "500 ", False), ("pn", "-0", False),
    ("pp", "00000000000000000000010000000", True),   # 10,000,000 with leading zeros
]
case("X08_big_numbers", """\
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
""",
     events=[
         ev("e1", "ua", "0" * 30 + "1791021600000", ingest=I),
         ev("e2", "ub", "18446745864731151616", ingest=I),
         ev("e3", "uc", "36893489938440703232", ingest=I),
         ev("e4", "ud", "9223373827876375808", ingest=I),
         ev("e5", "ue", "17910216000000000000000000000000000000", ingest=I),
     ] + [ev(f"p{n}", u, T, "purchase", "p1", price) for n, (u, price, _) in enumerate(X08_PRICES)]
       + [ev(f"w{n}", "whale", at("2026-10-05T10:00:00Z", n), "purchase", "p2", "10000000")
          for n in range(220)],
     clicks=["k1,whale,c_1,2026-10-05T09:00:00Z"],
     user_daily=[
         "pa,2026-10-03,1,1,10000000,shoes",
         "pc,2026-10-03,1,1,1,shoes",
         "pi,2026-10-03,1,1,500,shoes",
         "pp,2026-10-03,1,1,10000000,shoes",
         "ua,2026-10-03,1,1,0,",
         "whale,2026-10-05,1,220,2200000000,books",
     ],
     campaign_daily=[
         "c_1,search,2026-10-05,1,1,2200000000,1",
         "organic,organic,2026-10-03,5,4,20000501,0",
     ])

# --------------------------------------------------------------------------- X09
case("X09_dedup_order", """\
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
""",
     events={
         "part-00000.csv": [
             ev("e1", "ua", T, ingest="2026-10-03T10:05:00Z"),
             ev("e1", "ua", T) + ",x",
             ev("e2", "ub", T, ingest="2026-10-03T10:05:00Z"),
             ev("e2", '"ub', T),
             ev("e3", "ud", T, ingest="2026-10-03T10:05:00Z"),
             ev('"e3"', "uc", T),
             ev("e4", "uf", T, ingest="2026-10-03T10:05:00Z"),
             ev("e5", "ug", "2026-10-04T09:00:00Z", ingest="2026-10-04T10:00:02Z"),
             ev("e5", "ug", T, ingest="2026-10-04T10:00:01Z"),
             ev("e6", "uh", T, "purchase", "p1", "100", ingest="2026-10-03T10:02:00Z"),
             ev("e6", "uh", T, "Purchase", "p1", "100"),
             ev("e7", "ui", T, ingest="2026-10-03T10:02:00Z"),
             ev("e7", '""', T),
             ev("e8", "uj", T),
             '"e8","uj","2026-10-03T10:00:00Z","2026-10-03T10:01:00Z","view","p1",""',
             ev("E9", "uk", T),
             ev("e9", "uk", "2026-10-03T10:05:00Z"),
             ev("e10", "un", T, ingest="2026-10-03T10:03:00Z"),
             ev("e10", "un", T, ingest="2026-10-03T10:03:00Z"),
             ev("e10", "um", T),
         ],
         "part-00001.csv": [
             ev("e4", "ue", T),
         ],
     },
     user_daily=[
         "ua,2026-10-03,1,1,0,",
         "ub,2026-10-03,1,1,0,",
         "uc,2026-10-03,1,1,0,",
         "ue,2026-10-03,1,1,0,",
         "uj,2026-10-03,1,1,0,",
         "uk,2026-10-03,1,2,0,",
         "um,2026-10-03,1,1,0,",
     ],
     campaign_daily=["organic,organic,2026-10-03,7,0,0,0"])

# --------------------------------------------------------------------------- X10
case("X10_click_id_bytewise", """\
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
""",
     events=[ev(f"e{n}", u, f"2026-10-0{d}T10:00:00Z")
             for n, (u, d) in enumerate([("ua", 3), ("ub", 4), ("uc", 5), ("ud", 6), ("ue", 7), ("uf", 8)], 1)],
     clicks=[
         "k9,ua,c_1,2026-10-03T09:00:00Z",
         "k10,ua,c_2,2026-10-03T09:00:00Z",
         "k1,ub,c_2,2026-10-04T09:00:00Z",
         "K5,ub,c_1,2026-10-04T09:00:00Z",
         "k20,uc,c_2,2026-10-05T09:00:00Z",
         "k2,uc,c_1,2026-10-05T09:00:00Z",
         "k4,ud,c_2,2026-10-06T09:00:00Z",
         "k31,ud,c_1,1791277200000",
         "k5,ue,c_2,1791363600000",
         "k6,ue,c_1,1791363600001",
         "k70,uf,c_1,2026-10-08T09:00:00Z",
         "k7,uf,c_2,2026-10-08T11:00:00+02:00",
     ],
     user_daily=[f"{u},2026-10-0{d},1,1,0," for u, d in
                 [("ua", 3), ("ub", 4), ("uc", 5), ("ud", 6), ("ue", 7), ("uf", 8)]],
     campaign_daily=[
         "c_1,search,2026-10-03,0,0,0,1",
         "c_2,social,2026-10-03,1,0,0,1",
         "c_1,search,2026-10-04,1,0,0,1",
         "c_2,social,2026-10-04,0,0,0,1",
         "c_1,search,2026-10-05,1,0,0,1",
         "c_2,social,2026-10-05,0,0,0,1",
         "c_1,search,2026-10-06,1,0,0,1",
         "c_2,social,2026-10-06,0,0,0,1",
         "c_1,search,2026-10-07,1,0,0,1",
         "c_2,social,2026-10-07,0,0,0,1",
         "c_1,search,2026-10-08,0,0,0,1",
         "c_2,social,2026-10-08,1,0,0,1",
     ])

# --------------------------------------------------------------------------- X11
case("X11_invalid_click_no_shadow", """\
Invalid clicks are dropped before the latest click is chosen (K4, K7, K9, O5).

- ua (10:00 view, 10:10 purchase p1 1000): `k1` c_1 08:00 valid; `k2` c_999 09:00 unknown
  campaign → credited c_1, which assists alone. Purchasing, 1000.
- ub: `k3` c_1 08:00; `k4` `C_2` 09:00 (case variant, unknown) → c_1.
- uc: `k5` c_2 08:00; `k6` c_1 `09:00:00z` (bad ts) → c_2.
- ue: session starts Oct 4 00:10; `k9` c_1 Oct 3 23:00 → credited, on day Oct 4.
- uh: 10:00 view and a 10:05 purchase priced `19.99` (dropped); `k12` c_2 09:00 → c_2
  credited, but not a purchasing session.
""",
     events=[
         ev("a1", "ua", T), ev("a2", "ua", "2026-10-03T10:10:00Z", "purchase", "p1", "1000"),
         ev("b1", "ub", T),
         ev("c1", "uc", T),
         ev("e1", "ue", "2026-10-04T00:10:00Z"),
         ev("h1", "uh", T), ev("h2", "uh", "2026-10-03T10:05:00Z", "purchase", "p1", "19.99"),
     ],
     clicks=[
         "k1,ua,c_1,2026-10-03T08:00:00Z",
         "k2,ua,c_999,2026-10-03T09:00:00Z",
         "k3,ub,c_1,2026-10-03T08:00:00Z",
         "k4,ub,C_2,2026-10-03T09:00:00Z",
         "k5,uc,c_2,2026-10-03T08:00:00Z",
         "k6,uc,c_1,2026-10-03T09:00:00z",
         "k9,ue,c_1,2026-10-03T23:00:00Z",
         "k12,uh,c_2,2026-10-03T09:00:00Z",
     ],
     user_daily=[
         "ua,2026-10-03,1,2,1000,shoes",
         "ub,2026-10-03,1,1,0,",
         "uc,2026-10-03,1,1,0,",
         "ue,2026-10-04,1,1,0,",
         "uh,2026-10-03,1,1,0,",
     ],
     campaign_daily=[
         "c_1,search,2026-10-03,2,1,1000,2",
         "c_1,search,2026-10-04,1,0,0,1",
         "c_2,social,2026-10-03,2,0,0,2",
     ])

# --------------------------------------------------------------------------- X12
case("X12_session_drops", """\
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
""",
     events={
         "part-00000.csv": [
             ev("a1", "ua", T), ev("a2", "ua", "2026-10-03T10:25:00Z", ingest="2026-10-04T10:25:01Z"),
             ev("a3", "ua", "2026-10-03T10:50:00Z"),
             ev("b1", "ub", T), ev("b2", "ub", "2026-10-03T10:25:00Z", "View"),
             ev("b3", "ub", "2026-10-03T10:50:00Z"),
             ev("c1", "uc", "2026-10-03T10:25:00Z", ingest="2026-10-03T10:30:00Z"),
             ev("c1", "uc", T), ev("c2", "uc", "2026-10-03T10:50:00Z"),
             ev("d1", "ud", "1791021600500", ingest="2026-10-04T10:00:00Z"),
             ev("e1", "ue", "1791021600500", ingest="2026-10-04T10:00:01Z"),
             ev("f1", "uf", "1791071999999", ingest="2026-10-04T00:01:00Z"),
             ev("f2", "uf", "2026-10-04T00:20:00Z"),
             ev("g5", "ug", "2026-10-03T12:00:00Z"), ev("g4", "ug", "2026-10-03T11:00:00Z"),
             ev("g3", "ug", "2026-10-03T10:40:00Z"),
             ev("h1", "uh", T, ingest="2026-10-03T05:00:00Z"),
         ],
         "part-00001.csv": [
             ev("g2", "ug", "2026-10-03T10:20:00Z"), ev("g1", "ug", T),
         ],
     },
     user_daily=[
         "ua,2026-10-03,2,2,0,",
         "ub,2026-10-03,2,2,0,",
         "uc,2026-10-03,2,2,0,",
         "ud,2026-10-03,1,1,0,",
         "uf,2026-10-03,1,2,0,",
         "ug,2026-10-03,2,5,0,",
         "uh,2026-10-03,1,1,0,",
     ],
     campaign_daily=["organic,organic,2026-10-03,11,0,0,0"])

# --------------------------------------------------------------------------- X13
case("X13_top_category", """\
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
""",
     products=["p1,shoes", "p2,books", "p3,Toys"],
     events=[
         ev("a1", "ua", "2026-10-03T09:00:00Z", "purchase", "p1", "100"),
         ev("a2", "ua", "2026-10-03T09:05:00Z", "purchase", "p1", "100"),
         ev("a3", "ua", "2026-10-03T09:10:00Z", "purchase", "p1", "100"),
         ev("a4", "ua", "2026-10-03T14:00:00Z", "purchase", "p2", "400"),
         ev("b1", "ub", T, "purchase", "p9", "500"),
         ev("b2", "ub", "2026-10-03T10:05:00Z", "purchase", "p1", "500"),
         ev("b3", "ub", "2026-10-04T10:00:00Z", "purchase", "p9", "500"),
         ev("b4", "ub", "2026-10-04T10:05:00Z", "purchase", "p2", "300"),
         ev("c1", "uc", "2026-10-03T23:50:00Z", "purchase", "p1", "300"),
         ev("c2", "uc", "2026-10-04T00:10:00Z", "purchase", "p2", "500"),
         ev("c3", "uc", "2026-10-04T10:00:00Z", "purchase", "p1", "400"),
         ev("d1", "ud", T),
         ev("d2", "ud", "2026-10-04T10:00:00Z"),
         ev("d3", "ud", "2026-10-04T10:05:00Z", "purchase", "p1", "900", ingest="2026-10-05T10:05:01Z"),
         ev("e1", "ue", T, "purchase", "p2", "19.99"),
         ev("e2", "ue", "2026-10-03T10:05:00Z", "purchase", "p1", "100"),
         ev("f1", "uf", T, "purchase", "P1", "700"),
         ev("f2", "uf", "2026-10-03T10:05:00Z", "purchase", "p2", "600"),
         ev("g1", "ug", T, "purchase", "p3", "500"),
         ev("g2", "ug", "2026-10-03T10:05:00Z", "purchase", "p2", "500"),
         ev("h1", "uh", "2026-10-03T09:00:00Z", "purchase", "p2", "250"),
         ev("h2", "uh", "2026-10-03T12:00:00Z", "purchase", "p1", "250"),
     ],
     user_daily=[
         "ua,2026-10-03,2,4,700,books",
         "ub,2026-10-03,1,2,1000,shoes",
         "ub,2026-10-04,1,2,800,unknown",
         "uc,2026-10-03,1,2,800,books",
         "uc,2026-10-04,1,1,400,shoes",
         "ud,2026-10-03,1,1,0,",
         "ud,2026-10-04,1,1,0,",
         "ue,2026-10-03,1,1,100,shoes",
         "uf,2026-10-03,1,2,1300,unknown",
         "ug,2026-10-03,1,2,1000,Toys",
         "uh,2026-10-03,2,2,500,books",
     ],
     campaign_daily=[
         "organic,organic,2026-10-03,10,9,5400,0",
         "organic,organic,2026-10-04,3,2,1200,0",
     ])

# --------------------------------------------------------------------------- X14
chain = [(f"c{i:03d}", f"c{i + 1:03d}") if i % 2 == 0 else (f"c{i + 1:03d}", f"c{i:03d}")
         for i in range(299)]
random.Random(14).shuffle(chain)
chain_files = [[f"{a},{b}" for a, b in chain[k::3]] for k in range(3)]
case("X14_links", """\
Linked IDs (L3, L5, L7, L8, L9, C2, C9).

- Chain: `c000`…`c299` linked one after the next (alternating direction), the 299
  lines shuffled over 3 files. Events by `c299` (10:00) and `c150` (10:20) → one
  person, named `c000`, 1 session. Naive label propagation needs 299 rounds.
- Upper-case split: `ua1,ua2` and `UA2,ua3`. Events by `ua2` and `ua3` → two persons,
  `ua1` and `UA2` (`UA2` ≠ `ua2`).
- `\\r`: the line `ub1,ub2\\r` links `ub1` to `ub2\\r`, not `ub2`. Events by ub1 and ub2 →
  two persons, `ub1` and `ub2`.
- Space: ` uc1,uc2` links ` uc1` (leading space) to `uc2`. Events by uc1 and uc2 → `uc1`
  alone, and `uc2`'s person is named ` uc1` (a space sorts first).
- Kiosk: `pa1,pa2`, `pa2,kz`, `kz,pb1`, `pc1,kz` → one person, named `kz`. Events pa1
  10:00, pb1 10:20, pa2 10:45, pc1 11:10 → one session of 4 (gaps under 30 min, across
  three people); pc1 12:00 → a second session. Clicks: `kk1` by pb1 c_1 09:30 credits
  session 1 and qualifies for session 2; `kk2` by pa2 c_2 11:30 credits session 2. So
  c_1 assists both, c_2 assists session 2.
""",
     events=[
         ev("x1", "c299", T), ev("x2", "c150", "2026-10-03T10:20:00Z"),
         ev("y1", "ua2", T), ev("y2", "ua3", T),
         ev("z1", "ub1", T), ev("z2", "ub2", "2026-10-03T10:10:00Z"),
         ev("w1", "uc1", T), ev("w2", "uc2", "2026-10-03T10:10:00Z"),
         ev("q1", "pa1", T), ev("q2", "pb1", "2026-10-03T10:20:00Z"),
         ev("q3", "pa2", "2026-10-03T10:45:00Z"), ev("q4", "pc1", "2026-10-03T11:10:00Z"),
         ev("q5", "pc1", "2026-10-03T12:00:00Z"),
     ],
     clicks=[
         "kk1,pb1,c_1,2026-10-03T09:30:00Z",
         "kk2,pa2,c_2,2026-10-03T11:30:00Z",
     ],
     links={
         "part-00000.csv": ["ua1,ua2", "UA2,ua3", "ub1,ub2\r", " uc1,uc2",
                            "pa1,pa2", "pa2,kz", "kz,pb1", "pc1,kz"] + chain_files[0],
         "part-00001.csv": chain_files[1],
         "part-00002.csv": chain_files[2],
     },
     user_daily=[
         " uc1,2026-10-03,1,1,0,",
         "UA2,2026-10-03,1,1,0,",
         "c000,2026-10-03,1,2,0,",
         "kz,2026-10-03,2,5,0,",
         "ua1,2026-10-03,1,1,0,",
         "ub1,2026-10-03,1,1,0,",
         "ub2,2026-10-03,1,1,0,",
         "uc1,2026-10-03,1,1,0,",
     ],
     campaign_daily=[
         "c_1,search,2026-10-03,1,0,0,2",
         "c_2,social,2026-10-03,1,0,0,1",
         "organic,organic,2026-10-03,7,0,0,0",
     ])

# --------------------------------------------------------------------------- X15
case("X15_odd_window", """\
The rules don't promise a midnight start or a 14-day window (IN7). This one runs from
2026-10-01T12:00:00Z to 2026-10-04T00:00:00Z. The generator never does this, but the
manifest allows it, and a session's day is its UTC date whatever the window.

- ua Oct 1 11:59:59: before the window, dropped. ub Oct 1 12:00:00: kept, day Oct 1;
  `k1` c_1 at exactly window_start − 7 d (Sep 24 12:00) credits it.
- uc Oct 2 00:30 → day Oct 2 (a day index counted from window_start would say Oct 1).
- ud Oct 3 23:59:59 → kept, day Oct 3. ue Oct 4 00:00 → at window_end, dropped.
- uf Oct 1 23:50 + Oct 2 00:10 → one session, day Oct 1.
""",
     window=("2026-10-01T12:00:00Z", "2026-10-04T00:00:00Z"),
     events=[
         ev("e1", "ua", "2026-10-01T11:59:59Z"),
         ev("e2", "ub", "2026-10-01T12:00:00Z"),
         ev("e3", "uc", "2026-10-02T00:30:00Z"),
         ev("e4", "ud", "2026-10-03T23:59:59Z"),
         ev("e5", "ue", "2026-10-04T00:00:00Z"),
         ev("e6", "uf", "2026-10-01T23:50:00Z"),
         ev("e7", "uf", "2026-10-02T00:10:00Z"),
     ],
     clicks=["k1,ub,c_1,2026-09-24T12:00:00Z"],
     user_daily=[
         "ub,2026-10-01,1,1,0,",
         "uc,2026-10-02,1,1,0,",
         "ud,2026-10-03,1,1,0,",
         "uf,2026-10-01,1,2,0,",
     ],
     campaign_daily=[
         "c_1,search,2026-10-01,1,0,0,1",
         "organic,organic,2026-10-01,1,0,0,0",
         "organic,organic,2026-10-02,1,0,0,0",
         "organic,organic,2026-10-03,1,0,0,0",
     ])

# --------------------------------------------------------------------------- X16
case("X16_multi_file", """\
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
""",
     events={
         "part-00000.csv": [ev("e1", "u1", T), ev("e4", "u2", "2026-10-03T10:50:00Z"),
                            ev("e5", "u3", T, ingest="2026-10-03T10:05:00Z")],
         "part-00003.csv": [ev("e2", "u1", "2026-10-03T10:20:00Z", "purchase", "p1", "500")],
         "other.csv": [ev("e3", "u1", "2026-10-03T10:40:00Z"), ev("e5", "u4", T)],
         "part-00010.csv": [],
     },
     clicks={
         "part-00000.csv": ["k1,u2,c_1,2026-10-03T09:00:00Z"],
         "part-00001.csv": [],
         "extra.csv": ["k2,u1,c_2,2026-10-03T09:30:00Z", "k3,u4,c_1,2026-10-03T09:59:59Z"],
     },
     links={
         "part-00000.csv": [],
         "part-00001.csv": ["u2,u1"],
         "b.csv": ["u5,u6"],
     },
     user_daily=[
         "u1,2026-10-03,1,4,500,shoes",
         "u4,2026-10-03,1,1,0,",
     ],
     campaign_daily=[
         "c_1,search,2026-10-03,1,0,0,2",
         "c_2,social,2026-10-03,1,1,500,1",
     ])


# --------------------------------------------------------------------------- write
def write(path, header, lines):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as fh:  # binary: exact bytes, \n endings only
        fh.write((header + "\n" + "".join(line + "\n" for line in lines)).encode("utf-8"))


def files(spec):
    return spec if isinstance(spec, dict) else {"part-00000.csv": list(spec)}


def main():
    for c in CASES:
        root = os.path.join(HERE, c["name"])
        if os.path.isdir(root):
            shutil.rmtree(root)
        inp = os.path.join(root, "input")
        for folder, header in (("events", EV), ("clicks", CK), ("links", LK)):
            for name, lines in files(c[folder]).items():
                write(os.path.join(inp, folder, name), header, lines)
        write(os.path.join(inp, "products.csv"), "product_id,category", c["products"])
        write(os.path.join(inp, "campaigns.csv"), "campaign_id,channel", c["campaigns"])
        with open(os.path.join(inp, "manifest.json"), "wb") as fh:
            fh.write(('{\n  "window_start": "%s",\n  "window_end": "%s"\n}\n' % c["window"]).encode())
        write(os.path.join(root, "expected", "user_daily.csv"), UD, c["user_daily"])
        write(os.path.join(root, "expected", "campaign_daily.csv"), CD, c["campaign_daily"])
        with open(os.path.join(root, "README.md"), "wb") as fh:
            fh.write(f"# {c['name']}\n\n{c['readme']}".encode("utf-8"))
    print(f"wrote {len(CASES)} cases to {HERE}")


if __name__ == "__main__":
    main()
