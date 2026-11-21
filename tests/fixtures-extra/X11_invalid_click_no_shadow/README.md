# X11_invalid_click_no_shadow

Invalid clicks are dropped before the latest click is chosen (K4, K7, K9, O5).

- ua (10:00 view, 10:10 purchase p1 1000): `k1` c_1 08:00 valid; `k2` c_999 09:00 unknown
  campaign → credited c_1, which assists alone. Purchasing, 1000.
- ub: `k3` c_1 08:00; `k4` `C_2` 09:00 (case variant, unknown) → c_1.
- uc: `k5` c_2 08:00; `k6` c_1 `09:00:00z` (bad ts) → c_2.
- ue: session starts Oct 4 00:10; `k9` c_1 Oct 3 23:00 → credited, on day Oct 4.
- uh: 10:00 view and a 10:05 purchase priced `19.99` (dropped); `k12` c_2 09:00 → c_2
  credited, but not a purchasing session.
