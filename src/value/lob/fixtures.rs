//! Fixtures of the tests of large objects (RULES.md R-TEST-10): sources of
//! chunks in memory, as a table's or a scratch file's would be, counting
//! the chunks read; a temporary store in memory, counting the writers it
//! made; and values made of them.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{Chunk, Directory, Kept, Lob, LobBuilder, LobKind, LobSource, TempLobs, TempWriter, broken};
use crate::error::Result;

/// A source of chunks in memory, counting the chunks read.
#[derive(Debug)]
struct Chunks {
    directory: Directory,
    held: Vec<Arc<[u8]>>,
    reads: Arc<AtomicUsize>,
}

impl LobSource for Chunks {
    fn totals(&self) -> (u64, u64) {
        self.directory.totals()
    }
    fn directory(&self) -> Result<&Directory> {
        Ok(&self.directory)
    }
    fn chunk(&self, k: usize) -> Result<Arc<[u8]>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.held.get(k).cloned().ok_or_else(|| broken("no such chunk"))
    }
    fn kept(&self) -> Option<Kept> {
        None
    }
}

/// The large object of `kind` whose chunks are `pieces`, as given.
pub(crate) fn source(pieces: Vec<Vec<u8>>, kind: LobKind) -> Lob {
    let count = |p: &[u8]| match kind {
        LobKind::Character => std::str::from_utf8(p).unwrap().chars().count(),
        LobKind::Binary => p.len(),
    };
    let sizes = pieces.iter().map(|p| Chunk { octets: u32::try_from(p.len()).unwrap(), chars: u32::try_from(count(p)).unwrap() }).collect();
    let held = pieces.into_iter().map(Arc::from).collect();
    Lob::of_source(kind, Arc::new(Chunks { directory: Directory::new(sizes), held, reads: Arc::default() }))
}

/// The character large object of the chunks `texts`, as given.
pub(crate) fn texts(texts: &[&str]) -> Lob {
    source(texts.iter().map(|t| t.as_bytes().to_vec()).collect(), LobKind::Character)
}

/// A character large object of `text` in chunks of about `size` octets,
/// each cut at a character's boundary.
pub(crate) fn chunked_by(text: &str, size: usize) -> Lob {
    let mut pieces = Vec::new();
    let mut piece = String::new();
    for c in text.chars() {
        if piece.len().saturating_add(c.len_utf8()) > size {
            pieces.push(std::mem::take(&mut piece).into_bytes());
        }
        piece.push(c);
    }
    pieces.push(piece.into_bytes());
    source(pieces, LobKind::Character)
}

/// A binary large object of `octets` in chunks of `size`.
pub(crate) fn octets_by(octets: &[u8], size: usize) -> Lob {
    source(octets.chunks(size).map(<[u8]>::to_vec).collect(), LobKind::Binary)
}

/// A temporary store in memory, counting the writers it made and the
/// chunks read of what they made.
#[derive(Debug, Default)]
pub(crate) struct Temp {
    made: AtomicUsize,
    reads: Arc<AtomicUsize>,
}

impl Temp {
    /// How many large objects it was asked to write.
    pub(crate) fn made(&self) -> usize {
        self.made.load(Ordering::Relaxed)
    }

    /// How many chunks have been read of what it made.
    pub(crate) fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }
}

/// A large object being written in memory.
struct Writer {
    sizes: Vec<Chunk>,
    chunks: Vec<Arc<[u8]>>,
    reads: Arc<AtomicUsize>,
}

impl TempWriter for Writer {
    fn push(&mut self, chunk: &[u8], chars: u32) -> Result<()> {
        let octets = u32::try_from(chunk.len()).map_err(|_| broken("a chunk past 4G"))?;
        self.sizes.push(Chunk { octets, chars });
        self.chunks.push(Arc::from(chunk));
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Arc<dyn LobSource>> {
        Ok(Arc::new(Chunks { directory: Directory::new(self.sizes), held: self.chunks, reads: self.reads }))
    }
}

impl TempLobs for Temp {
    fn writer(&self) -> Result<Box<dyn TempWriter>> {
        self.made.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(Writer { sizes: Vec::new(), chunks: Vec::new(), reads: Arc::clone(&self.reads) }))
    }
}

/// The character large object of `text`, kept in chunks of `temp`.
pub(crate) fn chunked(temp: &Temp, text: &str) -> Lob {
    let mut b = LobBuilder::new(LobKind::Character, Some(temp));
    b.push_str(text).unwrap();
    b.finish().unwrap()
}

/// The whole text of `lob`.
pub(crate) fn text(lob: &Lob) -> String {
    String::from_utf8(lob.reader().read(0, lob.octets()).unwrap()).unwrap()
}

/// A character of `per` octets in UTF-8.
pub(crate) fn wide(per: usize) -> char {
    match per {
        1 => 'a',
        2 => 'é',
        3 => '€',
        _ => '😀',
    }
}
