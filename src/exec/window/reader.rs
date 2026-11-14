//! \[DESIGN\] What a window's frames are found by, of each row of a
//! partition: its peer group's number, its range key and its calls'
//! arguments, held as rows are (`exec/held.rs`), and read by place by
//! readers each of their own segment. Every reader moves forward only, so
//! several read one partition spilled without reading a segment back for
//! each row.

use crate::bytes::{Put, Reader as Bytes};
use crate::error::Result;
use crate::exec::broken;
use crate::exec::{HeldRows, Holdable};
use crate::spill::{ListItem, Located, record_room, row_bytes};
use crate::storage::Row;
use crate::value::codec::{put_row, row};
use crate::value::types::TypeRegistry;

/// One row of a partition, as its frames are found: the number of its
/// peer group in the partition, from 0, then the values its window's
/// calls read (`exec/window.rs`).
#[derive(Debug, Clone)]
pub(in crate::exec) struct Placed {
    /// Its peer group's number.
    pub(in crate::exec) group: u64,
    /// Its range key, then each call's argument.
    pub(in crate::exec) values: Row,
}

impl ListItem for Placed {
    fn bytes(&self) -> usize {
        row_bytes(&self.values).saturating_add(size_of::<u64>())
    }
}

impl Holdable for Placed {
    fn record(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(record_room(size_of::<u64>(), self.values.len()));
        out.put_u64(self.group);
        put_row(&mut out, &self.values);
        out
    }
    fn read(record: &[u8], types: &TypeRegistry) -> Result<Placed> {
        let mut r = Bytes::new(record, "window row").with_types(types);
        let group = r.u64()?;
        let values = row(&mut r)?;
        r.finish()?;
        Ok(Placed { group, values })
    }
}

/// A reader of a partition's rows by place, the spilled segment it read
/// last kept.
#[derive(Default)]
pub(in crate::exec) struct Reader {
    window: Option<(usize, Vec<Placed>)>,
}

impl Reader {
    /// The `place`th of `held`: in memory, or of its segment, read back
    /// once it is not the one it read last.
    ///
    /// # Errors
    ///
    /// The spill area's, reading the segment; SQLCODE -910 past the last.
    pub(in crate::exec) fn get<'a>(&'a mut self, held: &'a mut HeldRows<Placed>, place: usize) -> Result<&'a Placed> {
        let spilled = match held.locate(place) {
            None => return Err(broken("a window's row past its partition's last")),
            Some(Located::Held(_)) => None,
            Some(Located::Spilled { segment, at }) => Some((segment, at)),
        };
        let Some((segment, at)) = spilled else {
            return match held.locate(place) {
                Some(Located::Held(item)) => Ok(item),
                Some(Located::Spilled { .. }) | None => Err(broken("a window's row moved")),
            };
        };
        if self.window.as_ref().is_none_or(|(read, _)| *read != segment) {
            self.window = Some((segment, held.segment(segment)?));
        }
        self.window.as_ref().and_then(|(_, items)| items.get(at)).ok_or_else(|| broken("a window's row not in its segment"))
    }
}
