# X20_revenue_over_i32

One person-day whose revenue passes 2^31 (O1, 64-bit sums): 216 purchases of 10,000,000 cents,
one per minute, one session. 216 x 10,000,000 = 2,160,000,000 > 2,147,483,647. Both tables must
hold the exact total. (An implementation with 32-bit revenue columns must fall back or widen.)
