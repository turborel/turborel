//! \[DESIGN\] (RULES.md D1226) The states of a set function of a table's
//! groups written to a spill and read back as they were: a column of
//! states as its kind writes it (`typed/written.rs`, `carried/written.rs`),
//! and states written by their accumulators as those bytes, each group's
//! one after another.

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::logical::AggCall;

use super::{Carried, Sent, Typed};

/// The tags of the states' forms, as written.
const TYPED: u8 = 0;
const CARRIED: u8 = 1;
const WRITTEN: u8 = 2;

impl Sent {
    /// Appends the states of the groups at `places`: whether it could
    /// write them.
    pub(in crate::exec::table) fn put_groups(&self, places: &[u32], out: &mut Vec<u8>) -> bool {
        match self {
            Sent::Typed(typed) => {
                out.put_u8(TYPED);
                typed.put_groups(places, out);
                true
            }
            Sent::Carried(carried) => {
                out.put_u8(CARRIED);
                carried.put_groups(places, out)
            }
            Sent::Written { .. } => {
                out.put_u8(WRITTEN);
                let mut state = Vec::new();
                for &i in places {
                    state.clear();
                    out.put_u8(u8::from(self.write(crate::storage::mask::index(i), &mut state)));
                    out.put_bytes(&state);
                }
                true
            }
        }
    }

    /// Reads the states of `len` groups of `agg` `put_groups` wrote.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the bytes are not such states.
    pub(in crate::exec::table) fn read(reader: &mut Reader<'_>, len: usize, agg: &AggCall) -> Result<Sent> {
        match reader.u8()? {
            TYPED => Ok(Sent::Typed(Typed::read(reader, len, agg)?)),
            CARRIED => Ok(Sent::Carried(Carried::read(reader, len)?)),
            WRITTEN => {
                let (mut bytes, mut ends) = (Vec::new(), Vec::with_capacity(len));
                for _ in 0..len {
                    let has = reader.u8()? != 0;
                    let state = reader.bytes()?;
                    if has {
                        bytes.extend_from_slice(state);
                    }
                    ends.push(bytes.len());
                }
                Ok(Sent::Written { bytes, ends })
            }
            tag => Err(reader.corrupt(&format!("no states of a set function have tag {tag}"))),
        }
    }
}
