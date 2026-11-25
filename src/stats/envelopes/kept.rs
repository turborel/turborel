//! \[DESIGN\] A geometry column's envelope statistics as bytes (RULES.md
//! D1093), so an engine that keeps a table's statistics with its pages
//! keeps these with them.
//!
//! ```text
//! envelopes: frame u8 0, or u8 1 | lower x f64 | lower y f64 | upper x f64 |
//!            upper y f64; cells u32 | each cell's share f64 | width f64 |
//!            height f64; empty f64 | vertices f64 | clustering f64
//! ```
//!
//! Integers are little-endian, and a number is its IEEE-754 bits.

use super::{Cell, EnvelopeStats};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::spatial::Bounds;

/// The byte before a frame that is there.
const FRAMED: u8 = 1;

fn put_f64(out: &mut Vec<u8>, x: f64) {
    out.put_u64(x.to_bits());
}

fn f64_of(r: &mut Reader<'_>) -> Result<f64> {
    r.u64().map(f64::from_bits)
}

impl EnvelopeStats {
    /// Appends it to `out`, as `read` reads it.
    pub fn put(&self, out: &mut Vec<u8>) {
        match self.frame {
            Some(frame) => {
                out.put_u8(FRAMED);
                for x in frame.lower().into_iter().chain(frame.upper()) {
                    put_f64(out, x);
                }
            }
            None => out.put_u8(0),
        }
        out.put_u32(u32::try_from(self.cells.len()).unwrap_or(u32::MAX));
        let cells = self.cells.iter().flat_map(|cell| [cell.share, cell.width, cell.height]);
        for x in cells.chain([self.empty, self.vertices, self.clustering]) {
            put_f64(out, x);
        }
    }

    /// The statistics `put` wrote, read of `r`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not what `put` writes.
    pub fn read(r: &mut Reader<'_>) -> Result<EnvelopeStats> {
        let frame = if r.u8()? == 0 {
            None
        } else {
            let lower = [f64_of(r)?, f64_of(r)?];
            Some(Bounds::new(lower, [f64_of(r)?, f64_of(r)?]).map_err(|_| r.corrupt("a frame that is no envelope"))?)
        };
        let cells =
            (0..r.u32()?).map(|_| Ok(Cell { share: f64_of(r)?, width: f64_of(r)?, height: f64_of(r)? })).collect::<Result<Vec<Cell>>>()?;
        Ok(EnvelopeStats { frame, cells, empty: f64_of(r)?, vertices: f64_of(r)?, clustering: f64_of(r)? })
    }
}
