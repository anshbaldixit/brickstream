//! Shopping Sessions pipeline. Reads `$DATA_DIR`, writes `$OUTPUT_DIR/{user_daily,campaign_daily}`
//! (or the last two arguments, DATA OUT). Stage timings go to stderr as `[fast] stage=... ms=...`.
//!
//! Phase 1 builds the link table (union-find over every link ID), then parses every event and
//! click chunk in parallel, resolving users to persons. Phase 2 dedups events per
//! `hash(event_id)` partition in place. Phase 3 sessionizes each person per `hash(person)`
//! partition; each worker writes its own user_daily file. Phase 4 writes campaign_daily.

mod clicks;
mod dedup;
mod events;
mod input;
mod key;
mod links;
mod lookups;
mod output;
mod scan;
mod sessions;
mod table;
mod ts;
mod wc;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::Instant;

use mimalloc::MiMalloc;
use rayon::prelude::*;

use crate::clicks::ClickParser;
use crate::dedup::{ClChunk, DedupStats, PRec};
use crate::events::{EvChunk, EventParser, ParseStats};
use crate::input::{chunks, list_csv, open, total_bytes, Input};
use crate::lookups::{read_manifest, Campaigns, Products};
use crate::output::CampaignRow;
use crate::sessions::{Acc, Big, Ctx, EvSrc, Row};
use crate::wc::Lines;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

const CHUNK_BYTES: usize = 4 << 20;
/// Event chunks are larger: each chunk is one slice of every P1 partition, so fewer chunks mean
/// fewer, longer slices for dedup.
const EVENT_CHUNK_BYTES: usize = 16 << 20;

struct Config {
    data: PathBuf,
    out: PathBuf,
    threads: usize,
    mode: String,
}

fn config() -> Config {
    let args: Vec<String> = std::env::args().collect();
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let (data, out) = if args.len() >= 3 {
        (PathBuf::from(&args[args.len() - 2]), PathBuf::from(&args[args.len() - 1]))
    } else {
        (PathBuf::from(env("DATA_DIR", "/data")), PathBuf::from(env("OUTPUT_DIR", "/output")))
    };
    let threads = env("NCPU", "")
        .parse::<usize>()
        .ok()
        .filter(|&n| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(8, |n| n.get()))
        .min(32);
    Config { data, out, threads, mode: env("FAST_MODE", "full") }
}

/// Process CPU time (user + system) in ms and peak RSS in MB.
fn usage() -> (u64, u64) {
    // SAFETY: getrusage only writes the struct we pass.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let ms = |t: libc::timeval| t.tv_sec as u64 * 1000 + t.tv_usec as u64 / 1000;
    (ms(ru.ru_utime) + ms(ru.ru_stime), ru.ru_maxrss as u64 / 1024)
}

/// A tuning knob: `FAST_<name>` from the environment, else `d`.
fn tune(name: &str, d: usize) -> usize {
    std::env::var(format!("FAST_{name}")).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

pub fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Minor page faults, system CPU ms and total CPU ms so far, for the stage log.
fn faults() -> (u64, u64, u64) {
    // SAFETY: getrusage only writes the struct we pass.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let ms = |t: libc::timeval| t.tv_sec as u64 * 1000 + t.tv_usec as u64 / 1000;
    (ru.ru_minflt as u64, ms(ru.ru_stime), ms(ru.ru_stime) + ms(ru.ru_utime))
}

macro_rules! stage {
    ($name:expr, $t:expr $(, $k:literal = $v:expr)*) => {{
        let (flt, sys, cpu) = faults();
        eprintln!(concat!("[fast] stage={} ms={:.1}" $(, " ", $k, "={}")*, " minflt={} sys_ms={} cpu_now={}"), $name, ms($t) $(, $v)*, flt, sys, cpu);
    }};
}

fn main() {
    let cfg = config();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(cfg.threads).build().unwrap();
    // Run on a pool thread, so every parallel closure (even an inlined small one) has a thread index.
    pool.install(|| run(cfg));
}

fn run(cfg: Config) {
    let t_start = Instant::now();
    key::set_force_intern(std::env::var("FAST_FORCE_INTERN").is_ok_and(|v| v == "1"));
    events::PLAIN_SCATTER.store(tune("NO_WC", 0) == 1, Relaxed);
    let threads = cfg.threads;

    // Phase 0: the small inputs, then map and chunk every CSV.
    let t = Instant::now();
    let window = read_manifest(&cfg.data.join("manifest.json"));
    let (products, campaigns) =
        rayon::join(|| Products::read(&cfg.data.join("products.csv")), || Campaigns::read(&cfg.data.join("campaigns.csv")));
    let open_all = |dir: &str| -> Vec<Input> { list_csv(&cfg.data.join(dir)).par_iter().map(|p| open(p)).collect() };
    let (ev_files, (cl_files, ln_files)) =
        rayon::join(|| open_all("events"), || rayon::join(|| open_all("clicks"), || open_all("links")));
    let ev_chunks = chunks(&ev_files, 0, tune("EV_CHUNK_KB", EVENT_CHUNK_BYTES >> 10) << 10);
    let cl_chunks = chunks(&cl_files, 0, CHUNK_BYTES);
    let ln_chunks = chunks(&ln_files, 0, CHUNK_BYTES);
    let ev_bytes = total_bytes(&ev_files);
    let cl_bytes = total_bytes(&cl_files);
    stage!("setup", t, "threads" = threads, "event_files" = ev_files.len(), "event_bytes" = ev_bytes,
        "click_bytes" = cl_bytes, "link_bytes" = total_bytes(&ln_files), "chunks" = ev_chunks.len() + cl_chunks.len() + ln_chunks.len());

    if cfg.mode == "readtouch" {
        // Baseline for reading instead of mapping: pread every chunk in 1 MB pieces into a reused
        // per-thread buffer and count newlines.
        let t = Instant::now();
        let all: Vec<(&std::path::Path, usize, usize)> = ev_chunks.iter().map(|c| (ev_files[c.file as usize].path.as_path(), c.start, c.end))
            .chain(cl_chunks.iter().map(|c| (cl_files[c.file as usize].path.as_path(), c.start, c.end)))
            .chain(ln_chunks.iter().map(|c| (ln_files[c.file as usize].path.as_path(), c.start, c.end)))
            .collect();
        let lines: usize = all.par_iter().map_init(|| vec![0u8; 1 << 20], |buf, &(path, s, e)| {
            use std::os::unix::fs::FileExt;
            let f = std::fs::File::open(path).unwrap();
            let (mut at, mut n) = (s, 0usize);
            while at < e {
                let len = (e - at).min(buf.len());
                f.read_exact_at(&mut buf[..len], at as u64).unwrap();
                n += memchr::memchr_iter(b'\n', &buf[..len]).count();
                at += len;
            }
            n
        }).sum();
        stage!("readtouch", t, "lines" = lines);
        finish(t_start);
    }
    if cfg.mode == "touch" {
        // Baseline: read every byte once (count newlines), nothing else.
        let t = Instant::now();
        let all: Vec<(&[u8], usize, usize)> = ev_chunks.iter().map(|c| (&ev_files[c.file as usize].data[..], c.start, c.end))
            .chain(cl_chunks.iter().map(|c| (&cl_files[c.file as usize].data[..], c.start, c.end)))
            .chain(ln_chunks.iter().map(|c| (&ln_files[c.file as usize].data[..], c.start, c.end)))
            .collect();
        let lines: usize = all.par_iter().map(|&(d, s, e)| memchr::memchr_iter(b'\n', &d[s..e]).count()).sum();
        stage!("touch", t, "lines" = lines);
        finish(t_start);
    }

    // Phase 1: links (table + union-find) alongside every event and click chunk.
    let t = Instant::now();
    let est_lines = ev_bytes / 76 + 1;
    // P1 (event_id) partitions small enough that a dedup table (8 bytes x 2 x lines) stays in L2;
    // P2 (person) partitions from the same estimate, so clicks can be partitioned while parsed.
    let p1 = (est_lines / tune("P1_LINES", 10_000)).next_power_of_two().clamp(64, 8192);
    let shift1 = 64 - p1.trailing_zeros();
    let p2 = (est_lines / tune("P2_EVENTS", 32_000)).next_power_of_two().clamp(64, 16384);
    let shift2 = 64 - p2.trailing_zeros();
    let ev_parser = EventParser { window, products: &products };
    let cl_parser = ClickParser { window, campaigns: &campaigns };
    if cfg.mode == "scan" || cfg.mode == "noemit" {
        // Experiments: the scanner alone, or full line parsing without the bucket writes.
        let t = Instant::now();
        let n: u64 = ev_chunks.par_iter().map(|c| {
            let d: &[u8] = &ev_files[c.file as usize].data;
            let mut n = 0u64;
            if cfg.mode == "scan" {
                scan::for_each_line::<7, _>(d, c.start, c.end, |f| n += f[0].len() as u64);
            } else {
                let mut memo = ts::DayMemo::default();
                scan::for_each_line::<7, _>(d, c.start, c.end, |f| if let Some(r) = ev_parser.parse(f, &mut memo) { n += r.valid() as u64 });
            }
            n
        }).sum();
        stage!(cfg.mode.as_str(), t, "n" = n);
        finish(t_start);
    }
    // Links first, with every thread (union-find is parallel), so the table is ready for phase 2.
    let (link_table, link_stats) = links::build(&ln_files, &ln_chunks);
    // Timing experiment only (wrong output): lookups against an empty, cache-resident table.
    let link_table = if tune("NOLINK", 0) == 1 { links::LinkTable::empty() } else { link_table };
    stage!("links", t, "lines" = link_stats.parse.lines.lines, "malformed" = link_stats.parse.lines.malformed,
        "edges" = link_stats.edges, "ids" = link_table.ids, "persons" = link_table.persons,
        "parse_ms" = format!("{:.1}", link_stats.ms_parse), "insert_ms" = format!("{:.1}", link_stats.ms_insert),
        "union_ms" = format!("{:.1}", link_stats.ms_union), "names_ms" = format!("{:.1}", link_stats.ms_names), "compact_ms" = format!("{:.1}", link_stats.ms_compact));
    // Helper threads that unmap the inputs once they are parsed (0: keep them mapped until exit).
    let unmap_threads = tune("UNMAP_THREADS", 2);
    let (ev_stats, ev_out, cl_out) = {
        let t = Instant::now();
        let parsed: Vec<(EvChunk, Lines<PRec>, ParseStats)> = ev_chunks
            .par_iter()
            .map(|c| events::parse_chunk(&ev_parser, &link_table, &ev_files[c.file as usize].data, c.start, c.end, p1, shift1, p2, shift2))
            .collect();
        let mut ev_stats = ParseStats::default();
        for (_, _, st) in &parsed {
            ev_stats.add(*st);
        }
        let (ev_out, ev_events): (Vec<EvChunk>, Vec<Lines<PRec>>) = parsed.into_iter().map(|(c, e, _)| (c, e)).unzip();
        input::release(ev_files, unmap_threads);
        stage!("events_parse", t, "lines" = ev_stats.lines.lines, "malformed" = ev_stats.lines.malformed,
            "empty_id" = ev_stats.empty_id, "records" = ev_stats.records, "valid_lines" = ev_stats.valid, "p1" = p1);
        let t = Instant::now();
        let cl: Vec<(ClChunk, ParseStats)> = cl_chunks
            .par_iter()
            .map(|c| clicks::parse_chunk(&cl_parser, &link_table, &cl_files[c.file as usize].data, c.start, c.end, p2, shift2))
            .collect();
        let mut st = ParseStats::default();
        for (_, s) in &cl {
            st.add(*s);
        }
        stage!("clicks_parse", t, "lines" = st.lines.lines, "malformed" = st.lines.malformed,
            "empty_id" = st.empty_id, "valid" = st.valid);
        let cl_out: Vec<ClChunk> = cl.into_iter().map(|(c, _)| c).collect();
        input::release(cl_files, unmap_threads.min(1));
        input::release(ln_files, unmap_threads.min(1));
        (ev_stats, (ev_out, ev_events), cl_out)
    };
    let (ev_out, mut ev_events) = ev_out;
    stage!("phase1", t);
    if cfg.mode == "parse" {
        finish(t_start);
    }

    // Phase 2: dedup per event_id partition, marking the losers' PRecs in place.
    let t = Instant::now();
    let est_ev = ev_stats.valid as usize + 1;
    let gsize = (p1 / tune("GROUPS", 256)).max(1);
    let marks = dedup::Marks::new(&mut ev_events);
    let dstats = (0..p1 / gsize)
        .into_par_iter()
        .map(|g| dedup::group(g * gsize..(g + 1) * gsize, &ev_out, &marks))
        .reduce(DedupStats::default, |mut a, b| {
            a.add(b);
            a
        });
    drop(marks);
    stage!("dedup", t, "records" = dstats.records, "losers" = dstats.losers, "kept_invalid" = dstats.kept_invalid,
        "survivors" = dstats.survivors, "p2" = p2, "cpu_ms" = dstats.ns / 1_000_000);
    let cl_parts = cl_out;
    // Every chunk's purchases in one table; a chunk's PRec::pay is offset by where its part starts.
    let mut purchases: Vec<(u32, u32)> = Vec::new();
    let mut ev_src: Vec<EvSrc> = Vec::with_capacity(ev_out.len());
    for (c, recs) in ev_out.iter().zip(ev_events.iter()) {
        ev_src.push(EvSrc { recs, offs: &c.eoffs, pbase: purchases.len() as u32 });
        purchases.extend_from_slice(&c.purchases);
    }

    // Phase 3: sessionize, credit and assist per person; largest partitions first. Each worker
    // writes its own user_daily file as soon as the partitions run out, while others finish.
    let t = Instant::now();
    let days = window.days();
    let n_camp = campaigns.len();
    let ud = cfg.out.join("user_daily");
    let cd = cfg.out.join("campaign_daily");
    for dir in [&ud, &cd] {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("fast: cannot create {}: {e}", dir.display());
            std::process::exit(3);
        }
    }
    let ctx = Ctx { window, events: &ev_src, purchases: &purchases, clicks: &cl_parts, campaigns: n_camp, categories: products.categories.len(), days };
    let mut order: Vec<(usize, usize)> = (0..p2)
        .map(|q| (ev_src.iter().map(|e| (e.offs[q + 1] - e.offs[q]) as usize).sum::<usize>() + cl_parts.iter().map(|c| c.part(q).len()).sum::<usize>(), q))
        .collect();
    order.sort_unstable_by(|a, b| b.cmp(a));
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Worker>> = Mutex::new(Vec::new());
    rayon::scope(|s| {
        for w in 0..threads {
            let (ctx, order, next, results, ud, categories) = (&ctx, &order, &next, &results, &ud, &products.categories);
            s.spawn(move |_| {
                let mut rows: Vec<Row> = Vec::with_capacity(est_ev / 8 / threads + 16);
                let mut acc = vec![Acc::default(); (n_camp + 1) * days];
                let mut assisted = vec![0u32; (n_camp + 1) * days];
                let mut big = Vec::new();
                let mut sessions = 0u64;
                loop {
                    let i = next.fetch_add(1, Relaxed);
                    if i >= order.len() {
                        break;
                    }
                    sessions += sessions::partition(ctx, order[i].1, &mut rows, &mut acc, &mut assisted, &mut big);
                }
                let ms_sessions = ms(t);
                // The first worker always writes, so the table has a file even with no rows.
                if w == 0 || !rows.is_empty() {
                    output::write_user_daily(&ud.join(format!("part-{w:05}.parquet")), &rows, categories);
                }
                let user_sessions = rows.iter().map(|r| r.sessions as u64).sum();
                let user_revenue = rows.iter().map(|r| r.revenue).sum();
                let done = Worker { rows: rows.len(), acc, assisted, big, sessions, user_sessions, user_revenue, ms_sessions, ms_written: ms(t) };
                // The rows are written; freeing them would only cost time.
                std::mem::forget(rows);
                results.lock().unwrap().push(done);
            });
        }
    });
    let results = results.into_inner().unwrap();
    let sessions_total: u64 = results.iter().map(|r| r.sessions).sum();
    let user_rows: usize = results.iter().map(|r| r.rows).sum();
    let last = |f: fn(&Worker) -> f64| results.iter().map(f).fold(0.0, f64::max);
    stage!("sessions", t, "sessions" = sessions_total, "user_daily_rows" = user_rows,
        "first_done_ms" = format!("{:.1}", results.iter().map(|r| r.ms_sessions).fold(f64::MAX, f64::min)),
        "last_done_ms" = format!("{:.1}", last(|r| r.ms_sessions)), "last_written_ms" = format!("{:.1}", last(|r| r.ms_written)));
    let mut bigs: Vec<Big> = results.iter().flat_map(|r| r.big.iter().copied()).collect();
    bigs.sort_unstable_by(|a, b| b.events.cmp(&a.events));
    for b in bigs.iter().take(3) {
        let mut name = Vec::new();
        key::push_bytes(b.person, &mut name);
        eprintln!("[fast] big person={} events={} clicks={} sessions={} ms={:.1}",
            String::from_utf8_lossy(&name), b.events, b.clicks, b.sessions, b.micros as f64 / 1000.0);
    }

    // Phase 4: campaign_daily (user_daily is written by now).
    let t = Instant::now();
    let mut acc = vec![Acc::default(); (n_camp + 1) * days];
    let mut assisted = vec![0u64; (n_camp + 1) * days];
    for r in &results {
        for (a, b) in acc.iter_mut().zip(r.acc.iter()) {
            a.sessions += b.sessions;
            a.purchasing += b.purchasing;
            a.revenue += b.revenue;
        }
        for (a, &b) in assisted.iter_mut().zip(r.assisted.iter()) {
            *a += b as u64;
        }
    }
    let organic = b"organic".to_vec();
    let first_day = window.first_day();
    let mut camp_rows = Vec::new();
    for c in 0..=n_camp {
        let (id, channel) = if c == n_camp {
            (organic.clone(), organic.clone())
        } else {
            (campaigns.ids[c].clone(), campaigns.channels[c].clone())
        };
        for d in 0..days {
            let a = &acc[c * days + d];
            let assisted = assisted[c * days + d];
            if a.sessions > 0 || assisted > 0 {
                camp_rows.push(CampaignRow {
                    campaign: id.clone(),
                    channel: channel.clone(),
                    day: (first_day + d as i64) as i32,
                    sessions: a.sessions,
                    purchasing: a.purchasing,
                    revenue: a.revenue,
                    assisted,
                });
            }
        }
    }
    // Self-check (rules.md): sessions and revenue agree between the tables.
    let user_sessions: u64 = results.iter().map(|r| r.user_sessions).sum();
    let user_revenue: u64 = results.iter().map(|r| r.user_revenue).sum();
    let camp_sessions: u64 = camp_rows.iter().map(|r| r.sessions).sum();
    let camp_revenue: u64 = camp_rows.iter().map(|r| r.revenue).sum();
    if user_sessions != camp_sessions || user_revenue != camp_revenue {
        eprintln!("[fast] SELF-CHECK FAILED sessions {user_sessions} vs {camp_sessions}, revenue {user_revenue} vs {camp_revenue}");
    }
    output::write_campaign_daily(&cd.join("part-00000.parquet"), &camp_rows);
    stage!("write", t, "user_daily_rows" = user_rows, "campaign_daily_rows" = camp_rows.len());
    finish(t_start);
}

/// What one phase-3 worker hands back once its user_daily file is written.
struct Worker {
    rows: usize,
    acc: Vec<Acc>,
    assisted: Vec<u32>,
    big: Vec<Big>,
    sessions: u64,
    user_sessions: u64,
    user_revenue: u64,
    ms_sessions: f64,
    ms_written: f64,
}

/// Logs the totals and exits at once, without dropping anything (the kernel reclaims it all).
/// `_exit` also skips at-exit handlers: the allocator's would walk and release every heap page
/// first, on one thread. Every output file is complete by now (`fs::write` returns after its
/// last write), and stderr is unbuffered.
fn finish(t_start: Instant) -> ! {
    let (cpu_ms, rss_mb) = usage();
    let end_us = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros());
    stage!("total", t_start, "cpu_ms" = cpu_ms, "maxrss_mb" = rss_mb, "end_us" = end_us);
    if tune("SMAPS", 0) == 1 {
        if let Ok(s) = std::fs::read_to_string("/proc/self/smaps_rollup") {
            eprintln!("{s}");
        }
    }
    if tune("NORMAL_EXIT", 0) == 1 {
        // For profile-guided builds: an instrumented binary writes its profile in an at-exit handler.
        std::process::exit(0);
    }
    // SAFETY: _exit only ends the process.
    unsafe { libc::_exit(0) }
}
