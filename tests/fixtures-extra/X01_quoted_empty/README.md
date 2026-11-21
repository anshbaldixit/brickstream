# X01_quoted_empty

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
