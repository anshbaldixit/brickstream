//! The two output tables as Parquet (rules.md "Outputs"), written by hand.
//!
//! The files are deliberately simple: row groups of 16,384 rows with one page per column, PLAIN
//! encoding, no compression, no dictionary, no statistics. `day` is INT32 (DATE), strings are
//! BYTE_ARRAY (STRING/UTF8), other integers INT32 or INT64; `top_category` is OPTIONAL, its
//! definition levels one bit-packed run per page. A table with no rows still gets a file (schema,
//! no row groups).
//! The footer is Thrift compact protocol (parquet.thrift's FileMetaData).

use std::io::Write;
use std::path::Path;

use crate::key::{packed_bytes, push_bytes, INTERNED};
use crate::sessions::{Row, NO_CATEGORY};

/// Rows per row group (and page): 16,384 rows of `Row` stay in L2 while all columns are encoded.
const GROUP_ROWS: usize = 16_384;

// Thrift compact protocol type ids.
const T_I32: u8 = 5;
const T_I64: u8 = 6;
const T_BINARY: u8 = 8;
const T_LIST: u8 = 9;
const T_STRUCT: u8 = 12;

/// A minimal Thrift compact-protocol writer.
struct Thrift {
    b: Vec<u8>,
    stack: Vec<i16>,
    last: i16,
}

impl Thrift {
    fn new() -> Thrift {
        Thrift { b: Vec::with_capacity(512), stack: Vec::new(), last: 0 }
    }
    fn varint(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.b.push((v as u8) | 0x80);
            v >>= 7;
        }
        self.b.push(v as u8);
    }
    fn zigzag(v: i64) -> u64 {
        ((v << 1) ^ (v >> 63)) as u64
    }
    fn field(&mut self, id: i16, ty: u8) {
        let delta = id - self.last;
        if (1..=15).contains(&delta) {
            self.b.push(((delta as u8) << 4) | ty);
        } else {
            self.b.push(ty);
            self.varint(Self::zigzag(id as i64));
        }
        self.last = id;
    }
    fn i32(&mut self, id: i16, v: i32) {
        self.field(id, T_I32);
        self.varint(Self::zigzag(v as i64));
    }
    fn i64(&mut self, id: i16, v: i64) {
        self.field(id, T_I64);
        self.varint(Self::zigzag(v));
    }
    fn binary(&mut self, id: i16, v: &[u8]) {
        self.field(id, T_BINARY);
        self.varint(v.len() as u64);
        self.b.extend_from_slice(v);
    }
    /// A struct-valued field; close with `end`.
    fn begin(&mut self, id: i16) {
        self.field(id, T_STRUCT);
        self.stack.push(self.last);
        self.last = 0;
    }
    /// A struct element of a list; close with `end`.
    fn begin_element(&mut self) {
        self.stack.push(self.last);
        self.last = 0;
    }
    fn end(&mut self) {
        self.b.push(0);
        self.last = self.stack.pop().unwrap();
    }
    fn list(&mut self, id: i16, elem: u8, n: usize) {
        self.field(id, T_LIST);
        if n < 15 {
            self.b.push(((n as u8) << 4) | elem);
        } else {
            self.b.push(0xF0 | elem);
            self.varint(n as u64);
        }
    }
    fn i32_element(&mut self, v: i32) {
        self.varint(Self::zigzag(v as i64));
    }
    fn binary_element(&mut self, v: &[u8]) {
        self.varint(v.len() as u64);
        self.b.extend_from_slice(v);
    }
    /// The STOP of the outermost struct.
    fn finish(mut self) -> Vec<u8> {
        self.b.push(0);
        self.b
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Str,
    OptStr,
    Date,
    I32,
    I64,
}

impl Kind {
    fn physical(self) -> i32 {
        match self {
            Kind::Str | Kind::OptStr => 6, // BYTE_ARRAY
            Kind::Date | Kind::I32 => 1,   // INT32
            Kind::I64 => 2,                // INT64
        }
    }
}

/// Writes one Parquet file. `encode(column, r0, r1, out)` appends the PLAIN page data of rows
/// `r0..r1` (for `OptStr`: the definition levels with their length prefix, then the present values).
///
/// Rows go in row groups of `GROUP_ROWS`, one page per column each: every column of a group is
/// encoded while the group's rows are still in cache, and each group is written from a reused
/// buffer that is also still in cache, instead of a pass over all rows per column into one large
/// buffer of fresh memory.
pub fn write_parquet(path: &Path, cols: &[(&str, Kind)], rows: usize, encode: impl Fn(usize, usize, usize, &mut Vec<u8>)) {
    let mut file = std::fs::File::create(path).unwrap_or_else(|e| fail(path, e));
    let mut buf: Vec<u8> = Vec::with_capacity(GROUP_ROWS * 48 + 4096);
    buf.extend_from_slice(b"PAR1");
    let mut at = 0usize; // file offset of buf[0]
    // Per row group: its rows, then (offset, size) of each column chunk.
    let mut groups: Vec<(usize, Vec<(usize, usize)>)> = Vec::with_capacity(rows.div_ceil(GROUP_ROWS));
    let mut page: Vec<u8> = Vec::new();
    for r0 in (0..rows).step_by(GROUP_ROWS) {
        let r1 = (r0 + GROUP_ROWS).min(rows);
        let mut chunks = Vec::with_capacity(cols.len());
        for ci in 0..cols.len() {
            let start = at + buf.len();
            page.clear();
            encode(ci, r0, r1, &mut page);
            let mut h = Thrift::new();
            h.i32(1, 0); // DATA_PAGE
            h.i32(2, page.len() as i32);
            h.i32(3, page.len() as i32);
            h.begin(5); // DataPageHeader
            h.i32(1, (r1 - r0) as i32);
            h.i32(2, 0); // PLAIN
            h.i32(3, 3); // definition levels: RLE
            h.i32(4, 3); // repetition levels: RLE
            h.end();
            buf.extend_from_slice(&h.finish());
            buf.extend_from_slice(&page);
            chunks.push((start, at + buf.len() - start));
        }
        groups.push((r1 - r0, chunks));
        file.write_all(&buf).unwrap_or_else(|e| fail(path, e));
        at += buf.len();
        buf.clear();
    }
    let mut f = Thrift::new();
    f.i32(1, 1); // version
    f.list(2, T_STRUCT, cols.len() + 1); // schema
    f.begin_element();
    f.binary(4, b"schema");
    f.i32(5, cols.len() as i32);
    f.end();
    for &(name, kind) in cols {
        f.begin_element();
        f.i32(1, kind.physical());
        f.i32(3, (kind == Kind::OptStr) as i32); // REQUIRED / OPTIONAL
        f.binary(4, name.as_bytes());
        match kind {
            Kind::Str | Kind::OptStr => {
                f.i32(6, 0); // converted type UTF8
                f.begin(10); // logical type: STRING
                f.begin(1);
                f.end();
                f.end();
            }
            Kind::Date => {
                f.i32(6, 6); // converted type DATE
                f.begin(10); // logical type: DATE
                f.begin(6);
                f.end();
                f.end();
            }
            Kind::I32 | Kind::I64 => {}
        }
        f.end();
    }
    f.i64(3, rows as i64);
    f.list(4, T_STRUCT, groups.len()); // row_groups
    for (n, chunks) in &groups {
        f.begin_element();
        f.list(1, T_STRUCT, cols.len()); // columns
        for (&(name, kind), &(start, size)) in cols.iter().zip(chunks) {
            f.begin_element();
            f.i64(2, start as i64); // file_offset
            f.begin(3); // ColumnMetaData
            f.i32(1, kind.physical());
            f.list(2, T_I32, 2);
            f.i32_element(0); // PLAIN
            f.i32_element(3); // RLE
            f.list(3, T_BINARY, 1);
            f.binary_element(name.as_bytes());
            f.i32(4, 0); // UNCOMPRESSED
            f.i64(5, *n as i64);
            f.i64(6, size as i64);
            f.i64(7, size as i64);
            f.i64(9, start as i64); // data_page_offset
            f.end();
            f.end();
        }
        let total: usize = chunks.iter().map(|c| c.1).sum();
        f.i64(2, total as i64);
        f.i64(3, *n as i64);
        f.end();
    }
    let footer = f.finish();
    buf.extend_from_slice(&footer);
    buf.extend_from_slice(&(footer.len() as u32).to_le_bytes());
    buf.extend_from_slice(b"PAR1");
    file.write_all(&buf).unwrap_or_else(|e| fail(path, e));
}

fn fail(path: &Path, e: std::io::Error) -> ! {
    eprintln!("fast: cannot write {}: {e}", path.display());
    std::process::exit(3);
}

#[inline]
fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s);
}

/// Appends `f(row)` for every row: fixed-width values, without a capacity check per value.
#[inline(always)]
fn put_each<T, const N: usize>(out: &mut Vec<u8>, rs: &[T], f: impl Fn(&T) -> [u8; N]) {
    out.reserve(rs.len() * N);
    let (base, mut p) = (out.as_mut_ptr(), out.len());
    for r in rs {
        // SAFETY: `reserve` made room for N bytes per row.
        unsafe { base.add(p).cast::<[u8; N]>().write_unaligned(f(r)) };
        p += N;
    }
    // SAFETY: every byte up to `p` was written above.
    unsafe { out.set_len(p) };
}

/// Definition levels (bit width 1) as one bit-packed run, with the page's 4-byte length prefix.
fn put_levels<T>(out: &mut Vec<u8>, rs: &[T], present: impl Fn(&T) -> bool) {
    let groups = rs.len().div_ceil(8);
    let mut header = Vec::with_capacity(10);
    let mut v = ((groups as u64) << 1) | 1;
    while v >= 0x80 {
        header.push((v as u8) | 0x80);
        v >>= 7;
    }
    header.push(v as u8);
    out.extend_from_slice(&((header.len() + groups) as u32).to_le_bytes());
    out.extend_from_slice(&header);
    out.extend(rs.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |b, (j, r)| b | ((present(r) as u8) << j))));
}

/// The user_id column: each ID is a length prefix, then (for a packed key) 8 bytes written whole
/// and its 9th byte, of which only the ID's length is kept.
fn put_user_ids(out: &mut Vec<u8>, rs: &[Row]) {
    if rs.iter().any(|r| r.person & INTERNED != 0) {
        for r in rs {
            let at = out.len();
            out.extend_from_slice(&[0; 4]);
            push_bytes(r.person, out);
            let len = (out.len() - at - 4) as u32;
            out[at..at + 4].copy_from_slice(&len.to_le_bytes());
        }
        return;
    }
    out.reserve(rs.len() * 13);
    let (base, mut p) = (out.as_mut_ptr(), out.len());
    for r in rs {
        let (lo, ninth, n) = packed_bytes(r.person);
        // SAFETY: `reserve` made room for 13 bytes per row, and each row advances `p` by at most 13.
        unsafe {
            base.add(p).cast::<[u8; 4]>().write_unaligned((n as u32).to_le_bytes());
            base.add(p + 4).cast::<[u8; 8]>().write_unaligned(lo.to_le_bytes());
            *base.add(p + 12) = ninth;
        }
        p += 4 + n;
    }
    // SAFETY: every byte up to `p` was written above.
    unsafe { out.set_len(p) };
}

pub fn write_user_daily(path: &Path, rows: &[Row], categories: &[Vec<u8>]) {
    // rules.md: any integer width. Sessions and events per user-day stay far below 2^31; revenue
    // can pass it (a person's purchases are summed), so it stays INT64.
    let cols = [
        ("user_id", Kind::Str),
        ("day", Kind::Date),
        ("sessions", Kind::I32),
        ("events", Kind::I32),
        ("revenue_cents", Kind::I64),
        ("top_category", Kind::OptStr),
    ];
    let names: Vec<Vec<u8>> = categories.iter().map(|c| [&(c.len() as u32).to_le_bytes()[..], c].concat()).collect();
    write_parquet(path, &cols, rows.len(), |ci, r0, r1, out| {
        let rs = &rows[r0..r1];
        match ci {
            0 => put_user_ids(out, rs),
            1 => put_each(out, rs, |r| r.day.to_le_bytes()),
            2 => put_each(out, rs, |r| (r.sessions as i32).to_le_bytes()),
            3 => put_each(out, rs, |r| (r.events as i32).to_le_bytes()),
            4 => put_each(out, rs, |r| (r.revenue as i64).to_le_bytes()),
            _ => {
                put_levels(out, rs, |r| r.top != NO_CATEGORY);
                for r in rs.iter().filter(|r| r.top != NO_CATEGORY) {
                    out.extend_from_slice(&names[r.top as usize]);
                }
            }
        }
    });
}

pub struct CampaignRow {
    pub campaign: Vec<u8>,
    pub channel: Vec<u8>,
    pub day: i32,
    pub sessions: u64,
    pub purchasing: u64,
    pub revenue: u64,
    pub assisted: u64,
}

pub fn write_campaign_daily(path: &Path, rows: &[CampaignRow]) {
    let cols = [
        ("campaign_id", Kind::Str),
        ("channel", Kind::Str),
        ("day", Kind::Date),
        ("sessions", Kind::I64),
        ("purchasing_sessions", Kind::I64),
        ("revenue_cents", Kind::I64),
        ("assisted_sessions", Kind::I64),
    ];
    write_parquet(path, &cols, rows.len(), |ci, r0, r1, out| {
        let rs = &rows[r0..r1];
        match ci {
            0 => rs.iter().for_each(|r| put_str(out, &r.campaign)),
            1 => rs.iter().for_each(|r| put_str(out, &r.channel)),
            2 => rs.iter().for_each(|r| out.extend_from_slice(&r.day.to_le_bytes())),
            3 => rs.iter().for_each(|r| out.extend_from_slice(&(r.sessions as i64).to_le_bytes())),
            4 => rs.iter().for_each(|r| out.extend_from_slice(&(r.purchasing as i64).to_le_bytes())),
            5 => rs.iter().for_each(|r| out.extend_from_slice(&(r.revenue as i64).to_le_bytes())),
            _ => rs.iter().for_each(|r| out.extend_from_slice(&(r.assisted as i64).to_le_bytes())),
        }
    });
}
