//! \[DESIGN\] The records of large objects (RULES.md D79), `LobPut` and
//! `LobStale`, written and read: see the parent module's format.

use super::kinds::LOB_PUT;
use super::{Change, put_table, table};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::log::Lsn;

/// Writes `change`, a large object's record: LobPut or LobStale.
pub(super) fn put_lob_change(out: &mut Vec<u8>, change: &Change) {
    if let Change::LobPut { table, generation, id, body } = change {
        put_table(out, table);
        out.put_u32(*generation);
        out.put_u64(*id);
        out.put_bytes(body);
    } else if let Change::LobStale { through, table, ids } = change {
        out.put_u64(through.0);
        put_table(out, table);
        out.put_u32(u32::try_from(ids.len()).unwrap_or(u32::MAX));
        for &id in ids {
            out.put_u64(id);
        }
    }
}

/// A large object's record of `tag`, LOB_PUT or LOB_STALE, as
/// `put_lob_change` wrote it.
pub(super) fn lob_change(r: &mut Reader<'_>, tag: u8) -> Result<Change> {
    if tag == LOB_PUT {
        return Ok(Change::LobPut { table: table(r)?, generation: r.u32()?, id: r.u64()?, body: r.bytes()?.to_vec() });
    }
    let (through, table) = (Lsn(r.u64()?), table(r)?);
    let count = r.u32()?;
    Ok(Change::LobStale { through, table, ids: (0..count).map(|_| r.u64()).collect::<Result<_>>()? })
}
