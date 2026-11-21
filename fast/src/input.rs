//! Input files: list `*.csv`, map them into memory, and cut them into newline-aligned chunks so
//! every thread stays busy whatever the file sizes (one file can hold a quarter of all rows).

use std::fs::File;
use std::ops::Deref;
use std::path::{Path, PathBuf};

pub enum Data {
    Map(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl Deref for Data {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Data::Map(m) => m,
            Data::Owned(v) => v,
        }
    }
}

pub struct Input {
    pub path: PathBuf,
    pub data: Data,
}

/// A byte range of one file holding whole lines (the header is never inside a chunk).
#[derive(Clone, Copy, Debug)]
pub struct Chunk {
    pub file: u32,
    pub start: usize,
    pub end: usize,
}

/// Every `*.csv` regular file directly in `dir` (not starting with `.`), sorted by name.
/// A missing directory means no files.
pub fn list_csv(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.ends_with(".csv") && !name.starts_with('.')
        })
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    out.sort();
    out
}

/// Maps a file read-only, or reads it if mapping fails (some bind mounts can't be mapped).
pub fn open(path: &Path) -> Input {
    let data = match File::open(path) {
        Ok(file) => {
            let len = file.metadata().map(|m| m.len()).unwrap_or(0);
            if len == 0 {
                Data::Owned(Vec::new())
            } else {
                // SAFETY: the inputs are read-only for the whole run (/data is mounted read-only).
                match unsafe { memmap2::Mmap::map(&file) } {
                    Ok(m) => Data::Map(m),
                    Err(_) => Data::Owned(std::fs::read(path).unwrap_or_else(|e| fail(path, e))),
                }
            }
        }
        Err(e) => fail(path, e),
    };
    Input { path: path.to_path_buf(), data }
}

fn fail(path: &Path, e: std::io::Error) -> ! {
    eprintln!("fast: cannot read {}: {e}", path.display());
    std::process::exit(2);
}

/// Splits every file after its header line into chunks of about `target` bytes, each ending just
/// after a `\n` (or at the end of the file).
pub fn chunks(files: &[Input], file_offset: u32, target: usize) -> Vec<Chunk> {
    let mut out = Vec::new();
    for (i, input) in files.iter().enumerate() {
        let d: &[u8] = &input.data;
        let n = d.len();
        let mut pos = match memchr::memchr(b'\n', d) {
            Some(h) => h + 1,
            None => n, // only a header (or nothing)
        };
        while pos < n {
            let tentative = pos + target;
            let end = if tentative >= n {
                n
            } else {
                match memchr::memchr(b'\n', &d[tentative..]) {
                    Some(j) => tentative + j + 1,
                    None => n,
                }
            };
            out.push(Chunk { file: file_offset + i as u32, start: pos, end });
            pos = end;
        }
    }
    out
}

/// Unmaps (or frees) inputs that are no longer needed, on `threads` helper threads that are never
/// joined, in pieces of at most 64 MB. Tearing down the mappings of gigabytes of file pages takes
/// ~100 ms; done here it overlaps the later phases instead of running on one thread at exit.
/// With `threads == 0` the inputs stay mapped until exit.
pub fn release(files: Vec<Input>, threads: usize) {
    if threads == 0 {
        std::mem::forget(files);
        return;
    }
    const PIECE: usize = 64 << 20;
    let mut groups: Vec<Vec<(usize, usize)>> = vec![Vec::new(); threads];
    let mut owned = Vec::new();
    let mut i = 0;
    for f in files {
        match f.data {
            Data::Map(m) => {
                let (p, n) = (m.as_ptr() as usize, m.len());
                // The pieces below unmap it; dropping it too would unmap twice.
                std::mem::forget(m);
                for at in (0..n).step_by(PIECE) {
                    groups[i % threads].push((p + at, PIECE.min(n - at)));
                    i += 1;
                }
            }
            Data::Owned(v) => owned.push(v),
        }
    }
    let mut owned = Some(owned);
    for g in groups {
        let o = owned.take();
        std::thread::spawn(move || {
            for (p, n) in g {
                // SAFETY: each piece is a page-aligned part of a mapping that nothing references any
                // more (the `Input` was moved in here, and every record holds copies, not pointers).
                unsafe { libc::munmap(p as *mut libc::c_void, n) };
            }
            drop(o);
        });
    }
}

pub fn total_bytes(files: &[Input]) -> usize {
    files.iter().map(|f| f.data.len()).sum()
}
