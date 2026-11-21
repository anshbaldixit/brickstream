#!/usr/bin/env python3
"""
Shopping Sessions: DuckDB reference implementation.

Optimised for exact correctness against kit/rules.md, not for speed. Every step
is vectorised SQL; Python only orchestrates (file lists, the manifest, and the
fixpoint loop for linked IDs). Section comments name the rules.md section each
block implements.

Reads  $DATA_DIR    (default /data)
Writes $OUTPUT_DIR  (default /output): user_daily/ and campaign_daily/ as Parquet
Temp   $SCRATCH_DIR (default /scratch, or the OS temp dir when that is missing)
"""
import calendar
import glob
import json
import os
import re
import sys
import tempfile
import time

import duckdb

T0 = time.time()


def log(msg):
    print(f"[{time.time() - T0:7.2f}s] {msg}", file=sys.stderr, flush=True)


DATA = os.environ.get("DATA_DIR", "/data")
OUT = os.environ.get("OUTPUT_DIR", "/output")
if len(sys.argv) >= 3:  # optional `pipeline.py DATA OUT` (fixture runner, tools/diffseeds.py)
    DATA, OUT = sys.argv[-2], sys.argv[-1]
SCRATCH = os.environ.get("SCRATCH_DIR", "/scratch")
if not os.path.isdir(SCRATCH):
    SCRATCH = tempfile.gettempdir()
NCPU = int(os.environ.get("NCPU", os.cpu_count() or 1))
MEM_GB = int(os.environ.get("MEM_GB", "8"))

DAY_MS = 86_400_000
GAP_MS = 1_800_000          # Events step 5: a gap over 30 minutes starts a new session
CREDIT_MS = 7 * DAY_MS      # Clicks step 4: 604,800,000 ms
LATE_MS = 24 * 3_600_000    # Events step 4: over 24 h is late


def sql_str(s):
    """A SQL string literal, safe for paths with apostrophes (Builder's Hackathon)."""
    return "'" + str(s).replace("'", "''") + "'"


con = duckdb.connect()  # in-memory database
con.execute(f"SET threads = {NCPU}")
con.execute(f"SET memory_limit = '{max(1, int(MEM_GB * 0.8))}GB'")
con.execute(f"SET temp_directory = {sql_str(os.path.join(SCRATCH, 'duckdb_tmp'))}")
con.execute("SET preserve_insertion_order = false")


# ---------------------------------------------------------------------------
# Timestamps (rules.md "Timestamps"), used for the manifest window in Python.
# ---------------------------------------------------------------------------
F1_RE = re.compile(r"([0-9]{4})-([0-9]{2})-([0-9]{2})T([0-9]{2}):([0-9]{2}):([0-9]{2})Z", re.ASCII)


def f1_to_ms(s):
    m = F1_RE.fullmatch(s)
    if not m:
        raise ValueError(f"manifest timestamp is not F1: {s!r}")
    y, mo, d, h, mi, se = map(int, m.groups())
    if not (1 <= mo <= 12 and 1 <= d <= calendar.monthrange(y, mo)[1] and h <= 23 and mi <= 59 and se <= 59):
        raise ValueError(f"manifest timestamp is not a valid date: {s!r}")
    days = (calendar.timegm((y, mo, d, 0, 0, 0)) // 86400)
    return days * DAY_MS + (h * 3600 + mi * 60 + se) * 1000


with open(os.path.join(DATA, "manifest.json"), encoding="utf-8") as f:
    manifest = json.load(f)
WS = f1_to_ms(manifest["window_start"])
WE = f1_to_ms(manifest["window_end"])
CWS = WS - CREDIT_MS  # rules.md "Timestamps": a click's window starts 7 days earlier
log(f"window {manifest['window_start']} .. {manifest['window_end']}  ({WS} .. {WE})")


# ---------------------------------------------------------------------------
# SQL macros.
# ---------------------------------------------------------------------------
con.execute(r"""
-- rules.md "CSV dialect": a field is either unquoted with no quote anywhere in it,
-- or wrapped in quotes as a whole with no quote inside. Anything else is malformed.
CREATE MACRO fok(x) AS (strpos(x, '"') = 0 OR regexp_full_match(x, '"[^"]*"'));
-- The quotes are syntax; an empty value (quoted or not) is null. Spaces and \r stay.
CREATE MACRO fval(x) AS nullif(CASE WHEN starts_with(x, '"') THEN substr(x, 2, length(x) - 2) ELSE x END, '');

-- floor division (DuckDB's // truncates toward zero for negatives)
CREATE MACRO fdiv(a, b) AS (a - (((a % b) + b) % b)) // b;

-- days in a month, Gregorian leap years
CREATE MACRO dim(y, m) AS CASE
    WHEN m IN (1, 3, 5, 7, 8, 10, 12) THEN 31
    WHEN m IN (4, 6, 9, 11) THEN 30
    WHEN (y % 4 = 0 AND (y % 100 <> 0 OR y % 400 = 0)) THEN 29
    ELSE 28 END;

-- days since 1970-01-01 for a valid civil date (H. Hinnant's days_from_civil)
CREATE MACRO civ3(yp, mp, d) AS
    fdiv(yp, 400) * 146097
    + (yp - fdiv(yp, 400) * 400) * 365
    + (yp - fdiv(yp, 400) * 400) // 4
    - (yp - fdiv(yp, 400) * 400) // 100
    + (153 * mp + 2) // 5 + d - 1
    - 719468;
CREATE MACRO civ_days(y, m, d) AS
    civ3(y - CASE WHEN m <= 2 THEN 1 ELSE 0 END, CASE WHEN m > 2 THEN m - 3 ELSE m + 9 END, d);

-- rules.md "Timestamps", F1 and F2: split into parts (all ASCII-digit classes).
CREATE MACRO ts_parts(s) AS regexp_extract(s,
    '^([0-9]{4})-([0-9]{2})-([0-9]{2})T([0-9]{2}):([0-9]{2}):([0-9]{2})(Z|([+-])([0-9]{2}):([0-9]{2}))$',
    ['y', 'mo', 'd', 'h', 'mi', 'se', 'z', 'sg', 'oh', 'om']);

-- rules.md "Timestamps": s -> epoch ms (UTC), or NULL when bad.
-- F3 (digits only, leading zeros allowed): a value over 18 significant digits is
-- >= 1e18 ms, far outside any window, so it is NULL (bad) instead of overflowing.
CREATE MACRO ts_from(s, p) AS CASE
    WHEN s IS NULL THEN NULL
    WHEN regexp_full_match(s, '[0-9]+') THEN CASE
        WHEN ltrim(s, '0') = '' THEN 0::BIGINT
        WHEN length(ltrim(s, '0')) > 18 THEN NULL
        ELSE try_cast(ltrim(s, '0') AS BIGINT) END
    WHEN p.y = '' THEN NULL
    WHEN NOT (try_cast(p.mo AS BIGINT) BETWEEN 1 AND 12) THEN NULL
    WHEN NOT (try_cast(p.d AS BIGINT) BETWEEN 1 AND dim(try_cast(p.y AS BIGINT), try_cast(p.mo AS BIGINT))) THEN NULL
    WHEN try_cast(p.h AS BIGINT) > 23 OR try_cast(p.mi AS BIGINT) > 59 OR try_cast(p.se AS BIGINT) > 59 THEN NULL
    WHEN p.z <> 'Z' AND (try_cast(p.oh AS BIGINT) > 23 OR try_cast(p.om AS BIGINT) > 59) THEN NULL
    ELSE civ_days(try_cast(p.y AS BIGINT), try_cast(p.mo AS BIGINT), try_cast(p.d AS BIGINT)) * 86400000
         + (try_cast(p.h AS BIGINT) * 3600 + try_cast(p.mi AS BIGINT) * 60 + try_cast(p.se AS BIGINT)) * 1000
         - CASE WHEN p.z = 'Z' THEN 0
                ELSE (CASE WHEN p.sg = '-' THEN -1 ELSE 1 END)
                     * (try_cast(p.oh AS BIGINT) * 60 + try_cast(p.om AS BIGINT)) * 60000 END
    END;

-- rules.md "Events" step 3: a purchase price is ASCII digits only, value 1..10,000,000.
-- Returns the value, or NULL when the string is not digits only or too long to be <= 10M.
-- All zeros ('0', '000') is the value 0, which then fails the 1..10M check.
CREATE MACRO price_val(s) AS CASE
    WHEN s IS NULL OR NOT regexp_full_match(s, '[0-9]+') THEN NULL
    WHEN ltrim(s, '0') = '' THEN 0::BIGINT
    WHEN length(ltrim(s, '0')) > 8 THEN NULL
    ELSE try_cast(ltrim(s, '0') AS BIGINT) END;
""")


# ---------------------------------------------------------------------------
# rules.md "Input files" + "CSV dialect": read raw lines ourselves.
# Only \n ends a line; the first line of each file is its header; every other
# line (including an empty one, and one equal to the header) is a data line.
# ---------------------------------------------------------------------------
def csv_files(sub):
    return sorted(p for p in glob.glob(os.path.join(glob.escape(DATA), sub, "*.csv")) if os.path.isfile(p))


def load_csv(name, files, cols):
    n = len(cols)
    if files:
        con.execute(f"""
            CREATE TEMP TABLE raw_{name} AS
            SELECT unnest(string_split(content, chr(10))[2:]) AS line FROM read_text(?)
        """, [files])
    else:
        con.execute(f"CREATE TEMP TABLE raw_{name} (line VARCHAR)")
    sel = ", ".join(f"fval(f[{i + 1}]) AS {c}" for i, c in enumerate(cols))
    # A line with the wrong number of fields, or a stray quote, is malformed: drop it.
    con.execute(f"""
        CREATE TEMP TABLE {name} AS
        SELECT {sel}
        FROM (SELECT string_split(line, ',') AS f FROM raw_{name})
        WHERE len(f) = {n} AND list_bool_and(list_transform(f, lambda x: fok(x)))
    """)
    con.execute(f"DROP TABLE raw_{name}")
    total = con.execute(f"SELECT count(*) FROM {name}").fetchone()[0]
    log(f"read {name}: {len(files)} file(s), {total} well-formed lines")


load_csv("events_raw", csv_files("events"),
         ["event_id", "user_id", "ts", "ingest_ts", "event_type", "product_id", "price_cents"])
load_csv("clicks_raw", csv_files("clicks"), ["click_id", "user_id", "campaign_id", "ts"])
load_csv("links_raw", csv_files("links"), ["user_id_a", "user_id_b"])
load_csv("products_raw", [os.path.join(DATA, "products.csv")], ["product_id", "category"])
load_csv("campaigns_raw", [os.path.join(DATA, "campaigns.csv")], ["campaign_id", "channel"])

# products.csv and campaigns.csv are clean with unique IDs; group anyway so a
# lookup can never multiply rows.
con.execute("""
    CREATE TEMP TABLE products AS
    SELECT product_id, min(category) AS category FROM products_raw
    WHERE product_id IS NOT NULL GROUP BY product_id
""")
con.execute("""
    CREATE TEMP TABLE campaigns AS
    SELECT campaign_id, min(channel) AS channel FROM campaigns_raw
    WHERE campaign_id IS NOT NULL GROUP BY campaign_id
""")


# ---------------------------------------------------------------------------
# rules.md "Linked IDs"
# ---------------------------------------------------------------------------
# Step 1: drop lines with an empty side (malformed ones are already gone).
# Step 2: every line joins its two IDs; build the undirected edge list.
con.execute("""
    CREATE TEMP TABLE edges AS
    SELECT DISTINCT a, b FROM (
        SELECT user_id_a AS a, user_id_b AS b FROM links_raw WHERE user_id_a IS NOT NULL AND user_id_b IS NOT NULL
        UNION ALL
        SELECT user_id_b AS a, user_id_a AS b FROM links_raw WHERE user_id_a IS NOT NULL AND user_id_b IS NOT NULL
    )
""")
# Step 3: name each person by the byte-wise smallest ID (DuckDB's default VARCHAR
# collation is binary). Min-label propagation with pointer jumping: labels only
# ever move to a smaller ID of the same component, so the loop terminates; at the
# fixpoint every edge has equal labels on both ends, so each component carries
# one label, and it must be its smallest member (that member can only hold itself).
con.execute("CREATE TEMP TABLE lbl AS SELECT DISTINCT a AS id, a AS label FROM edges")
rounds = 0
while True:
    rounds += 1
    con.execute("""
        CREATE OR REPLACE TEMP TABLE lbl_step AS
        SELECT l.id, least(l.label, coalesce(m.mn, l.label)) AS label
        FROM lbl l
        LEFT JOIN (SELECT e.a AS id, min(n.label) AS mn FROM edges e JOIN lbl n ON n.id = e.b GROUP BY e.a) m
          ON m.id = l.id
    """)
    con.execute("""
        CREATE OR REPLACE TEMP TABLE lbl_next AS
        SELECT s.id, least(s.label, p.label) AS label
        FROM lbl_step s JOIN lbl_step p ON p.id = s.label
    """)
    changed = con.execute("""
        SELECT count(*) FROM lbl_next n JOIN lbl l ON l.id = n.id WHERE n.label <> l.label
    """).fetchone()[0]
    con.execute("CREATE OR REPLACE TEMP TABLE lbl AS SELECT * FROM lbl_next")
    if changed == 0:
        break
con.execute("CREATE TEMP TABLE person_map AS SELECT id, label AS person FROM lbl")
n_ids, n_people = con.execute("SELECT count(*), count(DISTINCT person) FROM person_map").fetchone()
log(f"links: {n_ids} linked IDs in {n_people} people, {rounds} rounds")


# ---------------------------------------------------------------------------
# rules.md "Events, in this order"
# ---------------------------------------------------------------------------
# Step 1: parse (done above); drop lines whose event_id is empty.
# Step 2: dedup on event_id, keeping the smallest ingest_ts, BEFORE validation.
# (regex parts are materialised once per row; ts_from reads them many times)
con.execute("""
    CREATE TEMP TABLE ev_parts AS
    SELECT *, ts_parts(ingest_ts) AS ip, ts_parts(ts) AS tp FROM events_raw WHERE event_id IS NOT NULL
""")
con.execute("""
    CREATE TEMP TABLE ev_kept AS
    SELECT * FROM (SELECT *, ts_from(ingest_ts, ip) AS ing_ms FROM ev_parts)
    QUALIFY row_number() OVER (PARTITION BY event_id ORDER BY ing_ms NULLS LAST) = 1
""")
con.execute("DROP TABLE ev_parts")
# Step 3: validate the kept copy. Step 4: drop late events (over 24 h; exactly 24 h kept).
con.execute(f"""
    CREATE TEMP TABLE ev_valid AS
    SELECT event_id, user_id, ts_ms, event_type, product_id,
           CASE WHEN event_type = 'purchase' THEN price ELSE 0 END AS price
    FROM (
        SELECT *, ts_from(ts, tp) AS ts_ms, price_val(price_cents) AS price
        FROM ev_kept
    )
    WHERE user_id IS NOT NULL
      AND ts_ms IS NOT NULL AND ts_ms >= {WS} AND ts_ms < {WE}
      AND event_type IN ('view', 'cart', 'purchase')
      AND product_id IS NOT NULL
      AND (event_type <> 'purchase' OR (price IS NOT NULL AND price BETWEEN 1 AND 10000000))
      AND ing_ms IS NOT NULL AND ing_ms - ts_ms <= {LATE_MS}
""")
# rules.md "Linked IDs": an event belongs to its ID's person.
con.execute("""
    CREATE TEMP TABLE ev_p AS
    SELECT v.*, coalesce(pm.person, v.user_id) AS person
    FROM ev_valid v LEFT JOIN person_map pm ON pm.id = v.user_id
""")
log(f"events: {con.execute('SELECT count(*) FROM ev_kept').fetchone()[0]} after dedup, "
    f"{con.execute('SELECT count(*) FROM ev_p').fetchone()[0]} valid")

# Step 5: sessionize per person over DISTINCT timestamps: the gap is measured
# between distinct ts, so all events sharing a ts join the same session.
# Step 6: the session's day is the UTC date of its start.
con.execute(f"""
    CREATE TEMP TABLE pts AS
    SELECT person, ts_ms,
           max(CASE WHEN is_new THEN ts_ms END)
               OVER (PARTITION BY person ORDER BY ts_ms ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS s_start
    FROM (
        SELECT person, ts_ms,
               (lag(ts_ms) OVER (PARTITION BY person ORDER BY ts_ms) IS NULL
                OR ts_ms - lag(ts_ms) OVER (PARTITION BY person ORDER BY ts_ms) > {GAP_MS}) AS is_new
        FROM (SELECT DISTINCT person, ts_ms FROM ev_p)
    )
""")
con.execute(f"""
    CREATE TEMP TABLE ev_s AS
    SELECT e.person, e.ts_ms, e.event_type, e.product_id, e.price, t.s_start,
           (DATE '1970-01-01' + fdiv(t.s_start, {DAY_MS})::INTEGER) AS day
    FROM ev_p e JOIN pts t ON t.person = e.person AND t.ts_ms = e.ts_ms
""")
con.execute("""
    CREATE TEMP TABLE sess AS
    SELECT person, s_start, any_value(day) AS day,
           count(*) AS n_events,
           sum(CASE WHEN event_type = 'purchase' THEN price ELSE 0 END)::BIGINT AS revenue,
           bool_or(event_type = 'purchase') AS has_purchase
    FROM ev_s GROUP BY person, s_start
""")
log(f"sessions: {con.execute('SELECT count(*) FROM sess').fetchone()[0]}")


# ---------------------------------------------------------------------------
# rules.md "Clicks"
# ---------------------------------------------------------------------------
# Step 1: parse (done above); drop lines whose click_id is empty.
# Step 2: dedup on click_id; copies are identical, keep any one.
# Step 3: validate: non-empty user_id, ts valid and in [window_start - 7d, window_end),
#         campaign_id exactly in campaigns.csv.
con.execute("""
    CREATE TEMP TABLE clk_parts AS
    SELECT *, ts_parts(ts) AS tp FROM clicks_raw WHERE click_id IS NOT NULL
""")
con.execute(f"""
    CREATE TEMP TABLE clk AS
    SELECT c.click_id, c.campaign_id, c.ts_ms, coalesce(pm.person, c.user_id) AS person
    FROM (
        SELECT *, ts_from(ts, tp) AS ts_ms FROM clk_parts
        QUALIFY row_number() OVER (PARTITION BY click_id) = 1
    ) c
    LEFT JOIN person_map pm ON pm.id = c.user_id
    WHERE c.user_id IS NOT NULL
      AND c.ts_ms IS NOT NULL AND c.ts_ms >= {CWS} AND c.ts_ms < {WE}
      AND c.campaign_id IN (SELECT campaign_id FROM campaigns)
""")
log(f"clicks: {con.execute('SELECT count(*) FROM clk').fetchone()[0]} valid")

# Steps 4 and 7 share one candidate set: the person's valid clicks with
# session_start - 7 days <= click.ts <= session_start.
con.execute(f"""
    CREATE TEMP TABLE cand AS
    SELECT s.person, s.s_start, c.click_id, c.ts_ms AS c_ts, c.campaign_id
    FROM sess s JOIN clk c
      ON c.person = s.person AND c.ts_ms <= s.s_start AND c.ts_ms >= s.s_start - {CREDIT_MS}
""")
# Step 4: credit = latest ts; on a tie the smallest click_id (byte-wise).
# Step 5: sessions with no candidate are credited to organic (the LEFT JOIN below).
con.execute("""
    CREATE TEMP TABLE credit AS
    SELECT person, s_start, campaign_id FROM cand
    QUALIFY row_number() OVER (PARTITION BY person, s_start ORDER BY c_ts DESC, click_id ASC) = 1
""")
# Step 7: assists: every distinct campaign among the candidates, once per session.
con.execute("CREATE TEMP TABLE assist AS SELECT DISTINCT person, s_start, campaign_id FROM cand")
log(f"credit: {con.execute('SELECT count(*) FROM credit').fetchone()[0]} credited sessions, "
    f"{con.execute('SELECT count(*) FROM assist').fetchone()[0]} assists")


# ---------------------------------------------------------------------------
# rules.md "Outputs"
# ---------------------------------------------------------------------------
# user_daily: one row per person and day with a session. Step 7 of Events:
# a product missing from products.csv is 'unknown'. top_category: highest
# summed purchase price, ties to the byte-wise smallest name, null with no purchases.
con.execute("""
    CREATE TEMP TABLE user_daily AS
    WITH ud AS (
        SELECT person, day, count(*) AS sessions, sum(n_events) AS events, sum(revenue) AS revenue_cents
        FROM sess GROUP BY person, day
    ),
    cat AS (
        SELECT e.person, e.day, coalesce(p.category, 'unknown') AS category, sum(e.price) AS amt
        FROM ev_s e LEFT JOIN products p ON p.product_id = e.product_id
        WHERE e.event_type = 'purchase'
        GROUP BY 1, 2, 3
    ),
    top AS (
        SELECT person, day, category FROM cat
        QUALIFY row_number() OVER (PARTITION BY person, day ORDER BY amt DESC, category ASC) = 1
    )
    SELECT ud.person AS user_id, ud.day,
           ud.sessions::BIGINT AS sessions, ud.events::BIGINT AS events,
           ud.revenue_cents::BIGINT AS revenue_cents, top.category AS top_category
    FROM ud LEFT JOIN top ON top.person = ud.person AND top.day = ud.day
""")
# campaign_daily: one row per (campaign, day) with a credited or assisted session.
con.execute("""
    CREATE TEMP TABLE campaign_daily AS
    WITH cred AS (
        SELECT coalesce(cr.campaign_id, 'organic') AS campaign_id, s.day,
               count(*) AS sessions,
               count(*) FILTER (WHERE s.has_purchase) AS purchasing_sessions,
               sum(s.revenue) AS revenue_cents
        FROM sess s LEFT JOIN credit cr ON cr.person = s.person AND cr.s_start = s.s_start
        GROUP BY 1, 2
    ),
    asst AS (
        SELECT a.campaign_id, s.day, count(*) AS assisted_sessions
        FROM assist a JOIN sess s ON s.person = a.person AND s.s_start = a.s_start
        GROUP BY 1, 2
    ),
    j AS (
        SELECT coalesce(cred.campaign_id, asst.campaign_id) AS campaign_id,
               coalesce(cred.day, asst.day) AS day,
               coalesce(cred.sessions, 0) AS sessions,
               coalesce(cred.purchasing_sessions, 0) AS purchasing_sessions,
               coalesce(cred.revenue_cents, 0) AS revenue_cents,
               coalesce(asst.assisted_sessions, 0) AS assisted_sessions
        FROM cred FULL OUTER JOIN asst ON asst.campaign_id = cred.campaign_id AND asst.day = cred.day
    )
    SELECT j.campaign_id,
           CASE WHEN j.campaign_id = 'organic' THEN 'organic' ELSE c.channel END AS channel,
           j.day,
           j.sessions::BIGINT AS sessions,
           j.purchasing_sessions::BIGINT AS purchasing_sessions,
           j.revenue_cents::BIGINT AS revenue_cents,
           j.assisted_sessions::BIGINT AS assisted_sessions
    FROM j LEFT JOIN campaigns c ON c.campaign_id = j.campaign_id
""")

# rules.md "Outputs": *.parquet directly in each folder, day as DATE.
for table, cols in [
    ("user_daily", "user_id, day, sessions, events, revenue_cents, top_category"),
    ("campaign_daily", "campaign_id, channel, day, sessions, purchasing_sessions, revenue_cents, assisted_sessions"),
]:
    folder = os.path.join(OUT, table)
    os.makedirs(folder, exist_ok=True)
    path = os.path.join(folder, "part-0.parquet")
    con.execute(f"COPY (SELECT {cols} FROM {table}) TO {sql_str(path)} (FORMAT parquet)")
    log(f"wrote {table}: {con.execute(f'SELECT count(*) FROM {table}').fetchone()[0]} rows")

# rules.md self-check: summed sessions and revenue agree between the tables.
s1 = con.execute("SELECT coalesce(sum(sessions),0), coalesce(sum(revenue_cents),0) FROM user_daily").fetchone()
s2 = con.execute("SELECT coalesce(sum(sessions),0), coalesce(sum(revenue_cents),0) FROM campaign_daily").fetchone()
if s1 != s2:
    log(f"SELF-CHECK FAILED: user_daily {s1} vs campaign_daily {s2}")
    sys.exit(2)
log("done")
