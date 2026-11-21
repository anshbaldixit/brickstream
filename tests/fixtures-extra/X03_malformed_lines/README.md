# X03_malformed_lines

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
