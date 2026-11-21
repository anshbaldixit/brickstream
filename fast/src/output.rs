//! Phase 4: the two output tables as Parquet (rules.md "Outputs").
//!
//! Plain encoding, no compression, no dictionary, no statistics. `day` is INT32 (DATE), strings are
//! BYTE_ARRAY (UTF8), counts and sums are INT64. Every table gets at least one file, even when it
//! has no rows. Each writer is closed explicitly (the footer is written on close), because the
//! process exits without running destructors.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use parquet::basic::Compression;
use parquet::data_type::{ByteArray, ByteArrayType, Int32Type, Int64Type};
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;

use crate::key::push_bytes;
use crate::sessions::{Row, NO_CATEGORY};

const USER_DAILY: &str = "message user_daily {
  REQUIRED BYTE_ARRAY user_id (UTF8);
  REQUIRED INT32 day (DATE);
  REQUIRED INT64 sessions;
  REQUIRED INT64 events;
  REQUIRED INT64 revenue_cents;
  OPTIONAL BYTE_ARRAY top_category (UTF8);
}";

const CAMPAIGN_DAILY: &str = "message campaign_daily {
  REQUIRED BYTE_ARRAY campaign_id (UTF8);
  REQUIRED BYTE_ARRAY channel (UTF8);
  REQUIRED INT32 day (DATE);
  REQUIRED INT64 sessions;
  REQUIRED INT64 purchasing_sessions;
  REQUIRED INT64 revenue_cents;
  REQUIRED INT64 assisted_sessions;
}";

pub enum Col {
    Str(Vec<ByteArray>, Option<Vec<i16>>),
    I32(Vec<i32>),
    I64(Vec<i64>),
}

fn fail(path: &Path, e: impl std::fmt::Display) -> ! {
    eprintln!("fast: cannot write {}: {e}", path.display());
    std::process::exit(3);
}

/// Writes one Parquet file with one row group (none if there are no rows).
pub fn write_file(path: &Path, schema: &str, rows: usize, cols: Vec<Col>) {
    let props = Arc::new(
        WriterProperties::builder()
            .set_writer_version(WriterVersion::PARQUET_1_0)
            .set_compression(Compression::UNCOMPRESSED)
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::None)
            .build(),
    );
    let schema = Arc::new(parse_message_type(schema).unwrap_or_else(|e| fail(path, e)));
    let file = File::create(path).unwrap_or_else(|e| fail(path, e));
    let mut writer = SerializedFileWriter::new(file, schema, props).unwrap_or_else(|e| fail(path, e));
    if rows > 0 {
        let mut rg = writer.next_row_group().unwrap_or_else(|e| fail(path, e));
        for col in cols {
            let mut c = rg.next_column().unwrap_or_else(|e| fail(path, e)).expect("schema has this column");
            let r = match &col {
                Col::Str(v, defs) => c.typed::<ByteArrayType>().write_batch(v, defs.as_deref(), None),
                Col::I32(v) => c.typed::<Int32Type>().write_batch(v, None, None),
                Col::I64(v) => c.typed::<Int64Type>().write_batch(v, None, None),
            };
            r.unwrap_or_else(|e| fail(path, e));
            c.close().unwrap_or_else(|e| fail(path, e));
        }
        rg.close().unwrap_or_else(|e| fail(path, e));
    }
    writer.close().unwrap_or_else(|e| fail(path, e));
}

/// user_daily rows as one file.
pub fn write_user_daily(path: &Path, rows: &[Row], categories: &[Bytes]) {
    let mut buf: Vec<u8> = Vec::with_capacity(rows.len() * 10);
    let mut ends: Vec<usize> = Vec::with_capacity(rows.len());
    for r in rows {
        push_bytes(r.person, &mut buf);
        ends.push(buf.len());
    }
    let buf = Bytes::from(buf);
    let mut start = 0;
    let ids: Vec<ByteArray> = ends
        .iter()
        .map(|&e| {
            let b = ByteArray::from(buf.slice(start..e));
            start = e;
            b
        })
        .collect();
    let defs: Vec<i16> = rows.iter().map(|r| (r.top != NO_CATEGORY) as i16).collect();
    let tops: Vec<ByteArray> =
        rows.iter().filter(|r| r.top != NO_CATEGORY).map(|r| ByteArray::from(categories[r.top as usize].clone())).collect();
    let cols = vec![
        Col::Str(ids, None),
        Col::I32(rows.iter().map(|r| r.day).collect()),
        Col::I64(rows.iter().map(|r| r.sessions as i64).collect()),
        Col::I64(rows.iter().map(|r| r.events as i64).collect()),
        Col::I64(rows.iter().map(|r| r.revenue as i64).collect()),
        Col::Str(tops, Some(defs)),
    ];
    write_file(path, USER_DAILY, rows.len(), cols);
}

pub struct CampaignRow {
    pub campaign: Bytes,
    pub channel: Bytes,
    pub day: i32,
    pub sessions: u64,
    pub purchasing: u64,
    pub revenue: u64,
    pub assisted: u64,
}

pub fn write_campaign_daily(path: &Path, rows: &[CampaignRow]) {
    let cols = vec![
        Col::Str(rows.iter().map(|r| ByteArray::from(r.campaign.clone())).collect(), None),
        Col::Str(rows.iter().map(|r| ByteArray::from(r.channel.clone())).collect(), None),
        Col::I32(rows.iter().map(|r| r.day).collect()),
        Col::I64(rows.iter().map(|r| r.sessions as i64).collect()),
        Col::I64(rows.iter().map(|r| r.purchasing as i64).collect()),
        Col::I64(rows.iter().map(|r| r.revenue as i64).collect()),
        Col::I64(rows.iter().map(|r| r.assisted as i64).collect()),
    ];
    write_file(path, CAMPAIGN_DAILY, rows.len(), cols);
}
