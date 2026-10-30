//! \[DESIGN\] An image of a database as of a checkpoint (RULES.md D25):
//! what its catalog store records (`db/stores.rs`). Its definitions
//! as they are, not every one made (`db/snapshot.rs`), so it is as large as
//! they are, whatever made them; each table's and index's locator; the log
//! position the checkpoint holds every change before (`through`); the
//! position of the first record of the oldest transaction open then
//! (`start`), from which recovery reads the log; and the highest
//! transaction id or timestamp given. `clean` says the database was closed
//! there: nothing has changed since, so, the log ending there, it is
//! reopened with nothing replayed (`db/files.rs`). A
//! database opened records its image again, not clean, before anything
//! else runs.
//!
//! ```text
//! image:  format u8 (IMAGE_FORMAT), clean u8 (0 or 1), through u64,
//!         start u64, highest u64, snapshot (`db/snapshot.rs`),
//!         tables u32, each auth (str), ident (str), generation u32,
//!           locator (bytes),
//!         indexes u32, each name (str), its table's generation u32,
//!           locator (bytes)
//! ```
//!
//! Fields are written through `bytes.rs`: integers little-endian, strings
//! and bytes after their length.

use super::snapshot::Snapshot;
use crate::bytes::{Put, Reader};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::log::Lsn;
use crate::value::types::TypeRegistry;

/// The format of an image.
pub const IMAGE_FORMAT: u8 = 1;

/// \[DESIGN\] A database as of a checkpoint.
#[derive(Debug, Clone, Default)]
pub(super) struct Image {
    /// Whether it was closed there.
    pub(super) clean: bool,
    /// The log position the checkpoint holds every change before.
    pub(super) through: Lsn,
    /// Where recovery reads the log from: the first record of the oldest
    /// transaction open at the checkpoint, or `through`.
    pub(super) start: Lsn,
    /// The highest transaction id or timestamp it had given.
    pub(super) highest: u64,
    /// Its definitions, as they were (`db/snapshot.rs`).
    pub(super) snapshot: Snapshot,
    /// Each table's locator.
    pub(super) tables: Vec<(QualifiedName, u32, Vec<u8>)>,
    /// Each index's locator.
    pub(super) indexes: Vec<(String, u32, Vec<u8>)>,
}

impl Image {
    /// It in bytes.
    pub(super) fn encode(&self) -> Vec<u8> {
        let mut out = vec![IMAGE_FORMAT];
        out.put_u8(u8::from(self.clean));
        out.put_u64(self.through.0);
        out.put_u64(self.start.0);
        out.put_u64(self.highest);
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        self.snapshot.put(&mut out);
        out.put_u32(count(self.tables.len()));
        for (name, generation, locator) in &self.tables {
            out.put_str(&name.auth);
            out.put_str(&name.ident);
            out.put_u32(*generation);
            out.put_bytes(locator);
        }
        out.put_u32(count(self.indexes.len()));
        for (name, generation, locator) in &self.indexes {
            out.put_str(name);
            out.put_u32(*generation);
            out.put_bytes(locator);
        }
        out
    }

    /// The image `bytes` give.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not one of this format, whole.
    pub(super) fn decode(bytes: &[u8], types: &TypeRegistry) -> Result<Image> {
        let mut r = Reader::new(bytes, "catalog image").with_types(types);
        if r.u8()? != IMAGE_FORMAT {
            return Err(r.corrupt("its format is not known"));
        }
        let clean = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err(r.corrupt("its clean flag is neither 0 nor 1")),
        };
        let (through, start, highest) = (Lsn(r.u64()?), Lsn(r.u64()?), r.u64()?);
        let snapshot = Snapshot::get(&mut r)?;
        let tables = (0..r.u32()?)
            .map(|_| Ok((QualifiedName { auth: r.str()?, ident: r.str()? }, r.u32()?, r.bytes()?.to_vec())))
            .collect::<Result<_>>()?;
        let indexes = (0..r.u32()?).map(|_| Ok((r.str()?, r.u32()?, r.bytes()?.to_vec()))).collect::<Result<_>>()?;
        r.finish()?;
        Ok(Image { clean, through, start, highest, snapshot, tables, indexes })
    }
}

// Unit tests: an image reads back as written, and malformed ones are
// refused (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;

    fn image(clean: bool) -> Image {
        Image {
            clean,
            through: Lsn(77),
            start: Lsn(40),
            highest: 9,
            snapshot: Snapshot::default(),
            tables: vec![(QualifiedName { auth: "A".into(), ident: "T".into() }, 2, vec![1, 2, 3])],
            indexes: vec![("I".into(), 2, vec![4]), ("J".into(), 0, Vec::new())],
        }
    }

    /// Verifies: [DESIGN] RULES.md D25, an image: its flag, positions,
    /// highest id, definitions and locators read back as written.
    #[test]
    fn an_image_reads_back_as_written() {
        let read = Image::decode(&image(true).encode(), &TypeRegistry::default()).unwrap();
        assert_eq!((read.clean, read.through, read.start, read.highest), (true, Lsn(77), Lsn(40), 9));
        assert_eq!(format!("{:?}", read.snapshot), format!("{:?}", image(true).snapshot));
        assert_eq!((read.tables, read.indexes), (image(true).tables, image(true).indexes));
    }

    /// Verifies: [DESIGN] RULES.md D25: an image of an unknown format, a
    /// clean flag neither 0 nor 1, bytes past its end, or cut short, is
    /// refused with SQLCODE -916.
    #[test]
    fn malformed_images_are_refused() {
        let whole = image(false).encode();
        let mut flag = whole.clone();
        flag[1] = 2;
        let mut format = whole.clone();
        format[0] = 9;
        for bad in [format, flag, [whole.clone(), vec![0]].concat(), whole[..whole.len() - 1].to_vec()] {
            assert_eq!(Image::decode(&bad, &TypeRegistry::default()).err().map(|e| e.code), Some(Code::Corrupt));
        }
    }
}
