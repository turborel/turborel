//! \[DESIGN\] (RULES.md D1226) A column of carried values written to a
//! spill and read back as it was: each group's value, a null too, and
//! whether it has one yet. A value written by more than its bytes, as a
//! large object is, is not written, and the column's groups stay where
//! they are (`keys/written.rs`).

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::spill::heap_bytes;
use crate::value::codec::{put_value, value};

use super::super::super::keys::written_alone;
use super::Carried;

impl Carried {
    /// Appends the values of the groups at `places`: whether it could.
    pub(in crate::exec::table) fn put_groups(&self, places: &[u32], out: &mut Vec<u8>) -> bool {
        for g in places.iter().map(|&i| crate::storage::mask::index(i)) {
            let Some(v) = self.values.get(g).filter(|v| written_alone(v)) else { return false };
            out.put_u8(u8::from(self.held.get(g).copied().unwrap_or(false)));
            put_value(out, v);
        }
        true
    }

    /// Reads the values of `len` groups `put_groups` wrote.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the bytes are not such values.
    pub(in crate::exec::table) fn read(reader: &mut Reader<'_>, len: usize) -> Result<Carried> {
        let (mut values, mut held) = (Vec::with_capacity(len), Vec::with_capacity(len));
        for _ in 0..len {
            held.push(reader.u8()? != 0);
            values.push(value(reader)?);
        }
        let heap = values.iter().map(heap_bytes).sum();
        Ok(Carried { values, held, heap })
    }
}
