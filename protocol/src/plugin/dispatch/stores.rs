//! \[EXTENSION\] The calls of storage engines and index methods (RULES.md
//! D224, D226), each operation a method of the trait.

use turborel::bytes::{Put, Reader};
use turborel::plugin::payload::{self as p, engine, index};
use turborel::plugin::values::values;

use super::super::registry::{Plugin, find};
use super::super::serve::Cx;
use super::super::{Answer, PluginError, Search};
use super::{read, written};

/// A locator, read.
fn locator(r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    read(r, |r| r.bytes().map(<[u8]>::to_vec))
}

/// A storage engine's operation.
///
/// # Errors
///
/// The engine's, or SQLSTATE 38000 of an operation unknown.
pub(super) fn engine(plugin: &mut Plugin, object: &str, op: u8, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let e = find(&mut plugin.engines, object)?;
    let mut out = Vec::new();
    match op {
        p::ENGINE_CREATE => {
            let (table, columns) = read(r, |r| Ok((r.str()?, p::strings(r)?)))?;
            out.put_bytes(&e.create(&table, &columns)?);
        }
        p::ENGINE_OPEN => engine::put_opened(&mut out, &e.open(&locator(r)?)?),
        p::ENGINE_SCAN => {
            let (at, after, max) = read(r, engine::scan)?;
            let (rows, more) = e.scan(&at, after, max)?;
            out = written(|o| engine::put_identified(o, &rows))?;
            out.put_u8(u8::from(more));
        }
        p::ENGINE_GET => {
            let (at, ids) = read(r, engine::ids)?;
            let got = e.get(&at, &ids)?;
            out = written(|o| engine::put_maybes(o, &got))?;
        }
        p::ENGINE_APPLY => {
            let (at, changes) = read(r, engine::changes)?;
            let before = e.apply(&at, &changes)?;
            out = written(|o| engine::put_maybes(o, &before))?;
        }
        p::ENGINE_CHECKPOINT => {
            let at = locator(r)?;
            e.checkpoint(&at, read(r, super::u64_of)?)?;
        }
        p::ENGINE_DROP => e.drop_table(&locator(r)?)?,
        _ => return Err(PluginError::external(&format!("an engine has no operation {op}"))),
    }
    Ok(out)
}

/// An index method's operation.
///
/// # Errors
///
/// The method's, or SQLSTATE 38000 of an operation unknown.
pub(super) fn index(plugin: &mut Plugin, cx: &mut Cx<'_>, object: &str, op: u8, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let m = find(&mut plugin.methods, object)?;
    let mut out = Vec::new();
    match op {
        p::INDEX_CREATE => out.put_bytes(&m.create(&read(r, p::strings)?)?),
        p::INDEX_OPEN => {
            let (found, clean) = m.open(&locator(r)?)?;
            out.put_u8(u8::from(found));
            out.put_u8(u8::from(clean));
        }
        p::INDEX_WRITE => {
            let (at, writes) = read(r, index::writes)?;
            m.write(&at, &writes)?;
        }
        p::INDEX_SEARCH => {
            let at = locator(r)?;
            let search = read(r, |from| {
                Ok(Search { probe: index::probe(from)?, descending: from.u8()? != 0, cursor: from.bytes()?.to_vec(), max: from.u32()? })
            })?;
            cx.check()?;
            let (entries, next) = m.search(&at, &search)?;
            out = written(|o| index::put_entries(o, &entries))?;
            out.put_bytes(&next);
        }
        p::INDEX_ROWS => {
            let at = locator(r)?;
            let ids = m.rows(&at, &read(r, values)?)?;
            out.put_u32(u32::try_from(ids.len()).unwrap_or(u32::MAX));
            for id in ids {
                out.put_u64(id);
            }
        }
        p::INDEX_NEAREST => {
            let (at, near) = read(r, index::near)?;
            let found = m.nearest(&at, &near)?;
            out = written(|o| index::put_neighbours(o, &found))?;
        }
        p::INDEX_CHECKPOINT => m.checkpoint(&locator(r)?)?,
        p::INDEX_DROP => m.drop_index(&locator(r)?)?,
        _ => return Err(PluginError::external(&format!("an index method has no operation {op}"))),
    }
    Ok(out)
}
