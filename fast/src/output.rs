//! Phase 4: the two output tables as Parquet (rules.md "Outputs"), written by hand.
//!
//! The files are deliberately simple: one row group, PLAIN encoding, no compression, no dictionary,
//! no statistics, data pages of at most 65,536 rows. `day` is INT32 (DATE), strings are BYTE_ARRAY
//! (STRING/UTF8), counts and sums are INT64; `top_category` is OPTIONAL, its definition levels a
//! single bit-packed run. A table with no rows still gets a file (schema, no row groups).
//! The footer is Thrift compact protocol (parquet.thrift's FileMetaData).

use std::path::Path;

use crate::key::push_bytes;
use crate::sessions::{Row, NO_CATEGORY};

const PAGE_ROWS: usize = 65_536;

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
    I64,
}

impl Kind {
    fn physical(self) -> i32 {
        match self {
            Kind::Str | Kind::OptStr => 6, // BYTE_ARRAY
            Kind::Date => 1,               // INT32
            Kind::I64 => 2,                // INT64
        }
    }
}

/// Writes one Parquet file. `encode(column, r0, r1, out)` appends the PLAIN page data of rows
/// `r0..r1` (for `OptStr`: the definition levels with their length prefix, then the present values).
pub fn write_parquet(path: &Path, cols: &[(&str, Kind)], rows: usize, encode: impl Fn(usize, usize, usize, &mut Vec<u8>)) {
    let mut file: Vec<u8> = Vec::with_capacity(rows * 64 + 1024);
    file.extend_from_slice(b"PAR1");
    let mut chunks: Vec<(usize, usize)> = Vec::with_capacity(cols.len());
    let mut page: Vec<u8> = Vec::new();
    if rows > 0 {
        for ci in 0..cols.len() {
            let start = file.len();
            let mut r0 = 0;
            while r0 < rows {
                let r1 = (r0 + PAGE_ROWS).min(rows);
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
                file.extend_from_slice(&h.finish());
                file.extend_from_slice(&page);
                r0 = r1;
            }
            chunks.push((start, file.len() - start));
        }
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
            Kind::I64 => {}
        }
        f.end();
    }
    f.i64(3, rows as i64);
    if rows > 0 {
        f.list(4, T_STRUCT, 1); // row_groups
        f.begin_element();
        f.list(1, T_STRUCT, cols.len()); // columns
        for (ci, &(name, kind)) in cols.iter().enumerate() {
            let (start, size) = chunks[ci];
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
            f.i64(5, rows as i64);
            f.i64(6, size as i64);
            f.i64(7, size as i64);
            f.i64(9, start as i64); // data_page_offset
            f.end();
            f.end();
        }
        let total: usize = chunks.iter().map(|c| c.1).sum();
        f.i64(2, total as i64);
        f.i64(3, rows as i64);
        f.end();
    } else {
        f.list(4, T_STRUCT, 0);
    }
    let footer = f.finish();
    file.extend_from_slice(&footer);
    file.extend_from_slice(&(footer.len() as u32).to_le_bytes());
    file.extend_from_slice(b"PAR1");
    if let Err(e) = std::fs::write(path, &file) {
        eprintln!("fast: cannot write {}: {e}", path.display());
        std::process::exit(3);
    }
}

#[inline]
fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s);
}

/// Definition levels (bit width 1) as one bit-packed run, with the page's 4-byte length prefix.
fn put_levels(out: &mut Vec<u8>, present: impl Iterator<Item = bool>, n: usize) {
    let groups = n.div_ceil(8);
    let mut header = Vec::with_capacity(10);
    let mut v = ((groups as u64) << 1) | 1;
    while v >= 0x80 {
        header.push((v as u8) | 0x80);
        v >>= 7;
    }
    header.push(v as u8);
    out.extend_from_slice(&((header.len() + groups) as u32).to_le_bytes());
    out.extend_from_slice(&header);
    let at = out.len();
    out.resize(at + groups, 0);
    for (i, p) in present.enumerate() {
        out[at + i / 8] |= (p as u8) << (i % 8);
    }
}

pub fn write_user_daily(path: &Path, rows: &[Row], categories: &[Vec<u8>]) {
    let cols = [
        ("user_id", Kind::Str),
        ("day", Kind::Date),
        ("sessions", Kind::I64),
        ("events", Kind::I64),
        ("revenue_cents", Kind::I64),
        ("top_category", Kind::OptStr),
    ];
    write_parquet(path, &cols, rows.len(), |ci, r0, r1, out| {
        let rs = &rows[r0..r1];
        match ci {
            0 => {
                for r in rs {
                    let at = out.len();
                    out.extend_from_slice(&[0; 4]);
                    push_bytes(r.person, out);
                    let len = (out.len() - at - 4) as u32;
                    out[at..at + 4].copy_from_slice(&len.to_le_bytes());
                }
            }
            1 => rs.iter().for_each(|r| out.extend_from_slice(&r.day.to_le_bytes())),
            2 => rs.iter().for_each(|r| out.extend_from_slice(&(r.sessions as i64).to_le_bytes())),
            3 => rs.iter().for_each(|r| out.extend_from_slice(&(r.events as i64).to_le_bytes())),
            4 => rs.iter().for_each(|r| out.extend_from_slice(&(r.revenue as i64).to_le_bytes())),
            _ => {
                put_levels(out, rs.iter().map(|r| r.top != NO_CATEGORY), rs.len());
                for r in rs.iter().filter(|r| r.top != NO_CATEGORY) {
                    put_str(out, &categories[r.top as usize]);
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
