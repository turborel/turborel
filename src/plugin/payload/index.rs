//! \[DESIGN\] The payloads of an index method's calls (RULES.md D226,
//! D227). An index is named by the locator CREATE gave; an entry is a key,
//! its values, and a row id; entries are counted, so an insert of a pair
//! held already holds it twice, and a remove takes back one insert.
//!
//! ```text
//! CREATE      columns: count u32 | type str...                  -> locator bytes
//! OPEN        locator bytes                                     -> found u8 | clean u8
//! WRITE       locator bytes | count u32 | (insert u8 | id u64 | key)...   -> (nothing)
//! SEARCH      locator bytes | probe | descending u8 | cursor bytes | max u32
//!                                                               -> entries | cursor bytes
//! ROWS        locator bytes | key                               -> count u32 | id u64...
//! NEAREST     locator bytes | column u32 | value | metric str | first u32 | descending u8
//!                                                               -> count u32 | (distance f64 | id u64 | key)...
//! CHECKPOINT  locator bytes                                     -> (nothing)
//! DROP        locator bytes                                     -> (nothing)
//! key:        count u32 | value...
//! entries:    count u32 | (id u64 | key)...
//! probe:      u8 0 every entry
//!             u8 1 | op u8 | value                       the first column compared
//!             u8 2 | equal: count u32 | value... | u8 0, or u8 1 | op u8 | value
//!             u8 3 | equal: count u32 | value... | low op u8 | value | high op u8 | value
//! op:         0 =, 1 <>, 2 <, 3 >, 4 <=, 5 >=
//! ```
//!
//! WRITE marks the index not clean before it changes it, CHECKPOINT makes
//! it durable and clean: OPEN of an index not clean has it rebuilt from
//! its table. SEARCH gives at most `max` entries a call, then a cursor to
//! go on from, empty after the last; of a method that SUPPORTS RANGE, in
//! ascending order of key, or descending if asked. ROWS gives the ids of
//! the entries whose key equals the key given, a null equal to a null.
//! NEAREST gives at most `first` entries, nearest first by `metric`.

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::plugin::values::{put_value, put_values, value, values};
use crate::storage::RowId;
use crate::value::{CompOp, Value};

use super::count;

/// The byte of `op`.
pub fn op_byte(op: CompOp) -> u8 {
    match op {
        CompOp::Eq => 0,
        CompOp::Ne => 1,
        CompOp::Lt => 2,
        CompOp::Gt => 3,
        CompOp::Le => 4,
        CompOp::Ge => 5,
    }
}

/// The operator `r` reads.
///
/// # Errors
///
/// SQLCODE -916 if it is none.
pub fn op_of(r: &mut Reader<'_>) -> Result<CompOp> {
    Ok(match r.u8()? {
        0 => CompOp::Eq,
        1 => CompOp::Ne,
        2 => CompOp::Lt,
        3 => CompOp::Gt,
        4 => CompOp::Le,
        5 => CompOp::Ge,
        other => return Err(r.corrupt(&format!("no operator has byte {other}"))),
    })
}

/// A probe, as SEARCH carries it.
#[derive(Debug, Clone)]
pub enum Probed {
    /// Every entry.
    All,
    /// The first column compared.
    Compare(CompOp, Value),
    /// The first columns equal, then perhaps the next compared.
    Prefix(Vec<Value>, Option<(CompOp, Value)>),
    /// The first columns equal, the next between bounds.
    Range(Vec<Value>, (CompOp, Value), (CompOp, Value)),
}

/// `p`, written.
///
/// # Errors
///
/// As `put_value`.
pub fn put_probe(out: &mut Vec<u8>, p: &Probed) -> Result<()> {
    let compared = |to: &mut Vec<u8>, (op, v): &(CompOp, Value)| {
        to.put_u8(op_byte(*op));
        put_value(to, v)
    };
    match p {
        Probed::All => out.put_u8(0),
        Probed::Compare(op, v) => {
            out.put_u8(1);
            compared(out, &(*op, v.clone()))?;
        }
        Probed::Prefix(equal, then) => {
            out.put_u8(2);
            put_values(out, equal)?;
            out.put_u8(u8::from(then.is_some()));
            then.iter().try_for_each(|t| compared(out, t))?;
        }
        Probed::Range(equal, low, high) => {
            out.put_u8(3);
            put_values(out, equal)?;
            compared(out, low)?;
            compared(out, high)?;
        }
    }
    Ok(())
}

/// What `put_probe` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn probe(r: &mut Reader<'_>) -> Result<Probed> {
    let compared = |from: &mut Reader<'_>| Ok::<_, crate::error::SqlError>((op_of(from)?, value(from)?));
    Ok(match r.u8()? {
        0 => Probed::All,
        1 => {
            let (op, v) = compared(r)?;
            Probed::Compare(op, v)
        }
        2 => {
            let equal = values(r)?;
            let then = if r.u8()? == 0 { None } else { Some(compared(r)?) };
            Probed::Prefix(equal, then)
        }
        3 => Probed::Range(values(r)?, compared(r)?, compared(r)?),
        other => return Err(r.corrupt(&format!("no probe has byte {other}"))),
    })
}

/// A write of WRITE: an insert or a remove of an entry.
#[derive(Debug, Clone)]
pub struct Write {
    /// An insert, else a remove.
    pub insert: bool,
    /// The row.
    pub id: RowId,
    /// The key.
    pub key: Vec<Value>,
}

/// WRITE's payload.
///
/// # Errors
///
/// As `put_values`.
pub fn put_writes(out: &mut Vec<u8>, locator: &[u8], writes: &[Write]) -> Result<()> {
    out.put_bytes(locator);
    out.put_u32(count(writes.len()));
    for w in writes {
        out.put_u8(u8::from(w.insert));
        out.put_u64(w.id);
        put_values(out, &w.key)?;
    }
    Ok(())
}

/// What `put_writes` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn writes(r: &mut Reader<'_>) -> Result<(Vec<u8>, Vec<Write>)> {
    let locator = r.bytes()?.to_vec();
    let all = (0..r.u32()?).map(|_| Ok(Write { insert: r.u8()? != 0, id: r.u64()?, key: values(r)? })).collect::<Result<_>>()?;
    Ok((locator, all))
}

/// Entries, written.
///
/// # Errors
///
/// As `put_values`.
pub fn put_entries(out: &mut Vec<u8>, entries: &[(RowId, Vec<Value>)]) -> Result<()> {
    out.put_u32(count(entries.len()));
    for (id, key) in entries {
        out.put_u64(*id);
        put_values(out, key)?;
    }
    Ok(())
}

/// What `put_entries` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn entries(r: &mut Reader<'_>) -> Result<Vec<(RowId, Vec<Value>)>> {
    (0..r.u32()?).map(|_| Ok((r.u64()?, values(r)?))).collect()
}

/// What NEAREST asks.
#[derive(Debug, Clone)]
pub struct Near {
    /// The key's column measured.
    pub column: u32,
    /// The value measured from.
    pub value: Value,
    /// The metric, upper case.
    pub metric: String,
    /// The most entries given.
    pub first: u32,
    /// Whether the greatest measure comes first.
    pub descending: bool,
}

/// NEAREST's payload.
///
/// # Errors
///
/// As `put_value`.
pub fn put_near(out: &mut Vec<u8>, locator: &[u8], n: &Near) -> Result<()> {
    out.put_bytes(locator);
    out.put_u32(n.column);
    put_value(out, &n.value)?;
    out.put_str(&n.metric);
    out.put_u32(n.first);
    out.put_u8(u8::from(n.descending));
    Ok(())
}

/// What `put_near` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn near(r: &mut Reader<'_>) -> Result<(Vec<u8>, Near)> {
    let locator = r.bytes()?.to_vec();
    Ok((locator, Near { column: r.u32()?, value: value(r)?, metric: r.str()?, first: r.u32()?, descending: r.u8()? != 0 }))
}

/// NEAREST's answer, written.
///
/// # Errors
///
/// As `put_values`.
pub fn put_neighbours(out: &mut Vec<u8>, all: &[(f64, RowId, Vec<Value>)]) -> Result<()> {
    out.put_u32(count(all.len()));
    for (distance, id, key) in all {
        out.put_u64(distance.to_bits());
        out.put_u64(*id);
        put_values(out, key)?;
    }
    Ok(())
}

/// What `put_neighbours` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn neighbours(r: &mut Reader<'_>) -> Result<Vec<(f64, RowId, Vec<Value>)>> {
    (0..r.u32()?).map(|_| Ok((f64::from_bits(r.u64()?), r.u64()?, values(r)?))).collect()
}
