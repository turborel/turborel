//! \[DESIGN\] (RULES.md D1173) The columns a group's header was read for.
//! A header holds each column's chunks, each with its bounds and its
//! parts', and a table of a hundred columns, of wide strings among them,
//! takes over a megabyte a group in memory decoded, more than the cache
//! of headers holds for a large table, so each scan of it read them all
//! again. A header is read for the columns its reader asks for; every
//! other column's chunks are passed over, none of their bounds decoded,
//! and that column is not held. Asked of such a column, the group says
//! so, never that it has no chunks.

use super::{Group, Zone};
use crate::error::{Code, Result, SqlError};

/// A column's chunks asked of a group whose header was read without them.
fn unread() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a frozen row group's header was read without a column asked of it")
}

impl Group {
    /// The chunks of column `column`: none of a column added after the
    /// group was written, which holds none of its values.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its header was read without them.
    pub fn zones(&self, column: usize) -> Result<Option<&[Zone]>> {
        match self.columns.get(column) {
            None => Ok(None),
            Some(Some(zones)) => Ok(Some(zones)),
            Some(None) => Err(unread()),
        }
    }

    /// Every column's chunks, in order, then those of every lengths lane
    /// (RULES.md D1319).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its header was read without a column's.
    pub fn every_zone(&self) -> Result<impl Iterator<Item = &Zone>> {
        if self.columns.iter().any(Option::is_none) {
            return Err(unread());
        }
        Ok(self.columns.iter().chain(&self.lanes).flatten().flatten())
    }

    /// Whether its header was read for each of `wanted`, or for every
    /// column if none is given.
    pub fn holds(&self, wanted: Option<&[usize]>) -> bool {
        match wanted {
            Some(wanted) => wanted.iter().all(|&c| self.columns.get(c).is_none_or(Option::is_some)),
            None => self.columns.iter().all(Option::is_some),
        }
    }

    /// The columns its header was read for, with `more`, each once: what
    /// a header read again for them all reads.
    pub fn with_columns(&self, more: &[usize]) -> Vec<usize> {
        let mut all: Vec<usize> =
            self.columns.iter().enumerate().filter(|(_, z)| z.is_some()).map(|(c, _)| c).chain(more.iter().copied()).collect();
        all.sort_unstable();
        all.dedup();
        all
    }
}
