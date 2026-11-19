//! \[DESIGN\] The large objects and XML values a spilled record names but
//! does not hold (RULES.md D38, D79, D106). The log's form of a value names
//! a table's object by its id alone, which only the table of the row
//! holding it resolves, and writes any other kept apart as no octets. A
//! record of rows an operator or a statement holds past its work area
//! names each value so kept by what keeps it, after the values naming it:
//! octets held inline as they are, a slice of a table's object by the
//! table's tag and the object's id, and a slice of any other source, a
//! temporary object, by the source itself, which the spill holds.
//!
//! A spill so holds one source of each table its records name, which makes
//! a source of any other object of that table (`LobSource::sibling`), and
//! each other source they name: a handle a table, and one a temporary
//! object of a chunk or more, never one a row. A table's object read back
//! is valid as the value spilled was, while the statement's snapshot is
//! held, as no object a snapshot reads is freed (D79).
//!
//! ```text
//! named: 0 | 1, its values u32, then each value's parts u32, then each
//!          part: 0 (inline) characters u64, its octets, their count u32
//!          first | 1 (a table's) its tag u64, the object's id u64, octets
//!          u64 and characters u64, then the slice | 2 (a source's) where
//!          the spill holds it u64, then the slice | 3 (a reference) the
//!          id u64 | 4 (a locator) u32
//! slice: its first octet u64 and character u64, its octets u64 and
//!          characters u64
//! ```
//!
//! A value named is written in its place as a reference to the `n`th
//! value named, from 0, of its kind, octets and characters.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::storage::Row;
use crate::value::Value;
use crate::value::codec::{put_row, put_value, row, value};
use crate::value::lob::{Lob, LobSource, Part, Slice};
use crate::value::types::{MAX_TYPE_NESTING, Structured};
use crate::value::xml::XmlValue;

/// The forms of a part of a value named.
const INLINE: u8 = 0;
const TABLE: u8 = 1;
const SOURCE: u8 = 2;
const REFERENCE: u8 = 3;
const LOCATOR: u8 = 4;

/// The sources a spill's records name.
#[derive(Default)]
struct Sources {
    /// A source of each table named, by the table's tag: what makes a
    /// source of each of its objects named.
    tables: HashMap<u64, Arc<dyn LobSource>>,
    /// Each other source named, by where it is held, which it is held at
    /// while the spill is, so no other is.
    others: HashMap<u64, Arc<dyn LobSource>>,
}

/// \[DESIGN\] The sources of the large objects and XML values a spill's
/// records name, held while the spill is (RULES.md D38, D79). Clones share
/// them.
#[derive(Clone, Default)]
pub struct SpilledLobs(Arc<Mutex<Sources>>);

impl fmt::Debug for SpilledLobs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let held = self.held();
        f.debug_struct("SpilledLobs").field("tables", &held.tables.len()).field("others", &held.others.len()).finish()
    }
}

/// Whether `v`, inside `depth` values, holds a large object not held
/// inline, alone or in an XML value: what a record names.
fn apart(v: &Value, depth: usize) -> bool {
    match v {
        Value::Lob(l) => l.inline().is_none(),
        Value::Xml(x) => x.stored_lob().is_some_and(|l| l.inline().is_none()),
        Value::Array(parts) | Value::Multiset(parts) | Value::Row(parts) => {
            depth < MAX_TYPE_NESTING && parts.iter().any(|p| apart(p, depth.saturating_add(1)))
        }
        Value::Structured(s) => depth < MAX_TYPE_NESTING && s.attributes.iter().any(|p| apart(p, depth.saturating_add(1))),
        Value::Null
        | Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Extension(_) => false,
    }
}

/// `v`, inside `depth` values, each value made of values in it rebuilt of
/// its parts as `each` makes them.
fn rebuilt(v: &Value, depth: usize, each: &mut dyn FnMut(&Value, usize) -> Result<Value>) -> Result<Value> {
    let inner = depth.saturating_add(1);
    let mut all = |parts: &[Value]| parts.iter().map(|p| each(p, inner)).collect::<Result<Arc<[Value]>>>();
    Ok(match v {
        Value::Array(parts) => Value::Array(all(parts)?),
        Value::Multiset(parts) => Value::Multiset(all(parts)?),
        Value::Row(parts) => Value::Row(all(parts)?),
        Value::Structured(s) => Value::Structured(Arc::new(Structured { attributes: all(&s.attributes)?, ..(**s).clone() })),
        Value::Null
        | Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => v.clone(),
    })
}

/// A count, as a record writes it.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Where `source` is held, as a record names it.
fn place_of(source: &Arc<dyn LobSource>) -> u64 {
    u64::try_from(Arc::as_ptr(source).cast::<()>().addr()).unwrap_or(u64::MAX)
}

/// The values a record names, as they are written after it.
#[derive(Default)]
struct Naming {
    /// How many.
    count: u32,
    /// Their parts, as the module's documentation says.
    bytes: Vec<u8>,
}

impl SpilledLobs {
    /// None held yet.
    pub fn new() -> SpilledLobs {
        SpilledLobs::default()
    }

    /// What it holds, to change or read.
    fn held(&self) -> MutexGuard<'_, Sources> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Holds also what `other` holds: a spill whose records joined its own.
    pub fn absorb(&self, other: &SpilledLobs) {
        if Arc::ptr_eq(&self.0, &other.0) {
            return;
        }
        let theirs = other.held();
        let (tables, others) = (theirs.tables.clone(), theirs.others.clone());
        drop(theirs);
        let mut held = self.held();
        held.tables.extend(tables);
        held.others.extend(others);
    }

    /// Appends `row` to `out`, as the value codec writes one (`codec::
    /// put_row`), each large object and XML value not held inline named
    /// after it.
    pub fn put_row(&self, out: &mut Vec<u8>, row: &Row) {
        let mut naming = Naming::default();
        if row.iter().any(|v| apart(v, 0)) {
            let named: Row = row.iter().map(|v| self.named(v, 0, &mut naming)).collect();
            put_row(out, &named);
        } else {
            put_row(out, row);
        }
        naming.put(out);
    }

    /// The row `put_row` wrote, read by `r`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is not one; SQLCODE -910 if a source it names is
    /// not held.
    pub fn row(&self, r: &mut Reader<'_>) -> Result<Row> {
        let mut found = row(r)?;
        if let Some(named) = self.read_named(r)? {
            for v in &mut found {
                *v = attached(v, 0, &named)?;
            }
        }
        Ok(found)
    }

    /// Appends `v` to `out`, as the value codec writes one (`codec::
    /// put_value`), each large object and XML value not held inline in it
    /// named after it.
    pub fn put_value(&self, out: &mut Vec<u8>, v: &Value) {
        let mut naming = Naming::default();
        if apart(v, 0) {
            put_value(out, &self.named(v, 0, &mut naming));
        } else {
            put_value(out, v);
        }
        naming.put(out);
    }

    /// The value `put_value` wrote, read by `r`.
    ///
    /// # Errors
    ///
    /// As `row`.
    pub fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let found = value(r)?;
        match self.read_named(r)? {
            Some(named) => attached(&found, 0, &named),
            None => Ok(found),
        }
    }

    /// `row` as a record (`put_row`).
    pub fn record(&self, row: &Row) -> Vec<u8> {
        let mut out = Vec::with_capacity(super::record_room(1, row.len()));
        self.put_row(&mut out, row);
        out
    }

    /// The row of `record`, as `record` wrote it, its values of types
    /// beyond the standard found among `types`.
    ///
    /// # Errors
    ///
    /// As `row`.
    pub fn record_row(&self, record: &[u8], types: &crate::value::types::TypeRegistry) -> Result<Row> {
        let mut r = Reader::new(record, "spilled row").with_types(types);
        let found = self.row(&mut r)?;
        r.finish()?;
        Ok(found)
    }

    /// `v`, inside `depth` values, each large object and XML value not held
    /// inline in it written as a reference to its place in `naming`.
    fn named(&self, v: &Value, depth: usize, naming: &mut Naming) -> Value {
        if let Value::Lob(l) = v
            && l.inline().is_none()
        {
            return Value::Lob(self.name(l, naming));
        }
        if let Value::Xml(x) = v
            && let Some(l) = x.stored_lob()
            && l.inline().is_none()
        {
            return Value::Xml(XmlValue::stored(self.name(l, naming)));
        }
        if depth < MAX_TYPE_NESTING && apart(v, depth) {
            return rebuilt(v, depth, &mut |p, at| Ok(self.named(p, at, naming))).unwrap_or_else(|_| v.clone());
        }
        v.clone()
    }

    /// A reference to `l`, written to `naming`, its sources held.
    fn name(&self, l: &Lob, naming: &mut Naming) -> Lob {
        let n = u64::from(naming.count);
        naming.count = naming.count.saturating_add(1);
        let out = &mut naming.bytes;
        out.put_u32(count(l.parts().len()));
        for part in l.parts() {
            match part {
                Part::Inline(bytes, chars) => {
                    out.put_u8(INLINE);
                    out.put_u64(*chars);
                    out.put_bytes(bytes);
                }
                Part::Slice(s) => self.put_slice(s, out),
                Part::Reference(id) => {
                    out.put_u8(REFERENCE);
                    out.put_u64(*id);
                }
                Part::Locator(locator) => {
                    out.put_u8(LOCATOR);
                    out.put_u32(*locator);
                }
            }
        }
        Lob::of_reference(l.kind(), n, (l.octets(), l.chars()))
    }

    /// Writes `s` to `out`, its source held: by its table and id, if a
    /// table keeps it and makes another of its objects; else by itself.
    fn put_slice(&self, s: &Slice, out: &mut Vec<u8>) {
        let mut held = self.held();
        if let Some(kept) = s.source.kept() {
            let maker = held.tables.contains_key(&kept.table) || {
                let makes = s.source.sibling(kept.id, s.source.totals()).is_some();
                if makes {
                    held.tables.insert(kept.table, Arc::clone(&s.source));
                }
                makes
            };
            if maker {
                let (octets, chars) = s.source.totals();
                out.put_u8(TABLE);
                out.put_u64(kept.table);
                out.put_u64(kept.id);
                out.put_u64(octets);
                out.put_u64(chars);
                put_range(s, out);
                return;
            }
        }
        let place = place_of(&s.source);
        held.others.entry(place).or_insert_with(|| Arc::clone(&s.source));
        out.put_u8(SOURCE);
        out.put_u64(place);
        put_range(s, out);
    }

    /// The parts of each value named after what `r` read, none if none is.
    fn read_named(&self, r: &mut Reader<'_>) -> Result<Option<Vec<Vec<Part>>>> {
        match r.u8()? {
            0 => return Ok(None),
            1 => {}
            _ => return Err(r.corrupt("no such naming of values")),
        }
        let values = r.u32()?;
        let held = self.held();
        let mut named = Vec::new();
        for _ in 0..values {
            let parts = r.u32()?;
            let mut found = Vec::new();
            for _ in 0..parts {
                found.push(read_part(r, &held)?);
            }
            named.push(found);
        }
        Ok(Some(named))
    }
}

impl Naming {
    /// Appends what it names to `out`.
    fn put(self, out: &mut Vec<u8>) {
        if self.count == 0 {
            out.put_u8(0);
            return;
        }
        out.put_u8(1);
        out.put_u32(self.count);
        out.extend_from_slice(&self.bytes);
    }
}

/// Appends the range of its source `s` is to `out`.
fn put_range(s: &Slice, out: &mut Vec<u8>) {
    out.put_u64(s.octet);
    out.put_u64(s.char);
    out.put_u64(s.octets);
    out.put_u64(s.chars);
}

/// The part `r` reads, its source found among `held`.
fn read_part(r: &mut Reader<'_>, held: &Sources) -> Result<Part> {
    Ok(match r.u8()? {
        INLINE => {
            let chars = r.u64()?;
            Part::Inline(Arc::from(r.bytes()?), chars)
        }
        TABLE => {
            let (table, id, totals) = (r.u64()?, r.u64()?, (r.u64()?, r.u64()?));
            let source = held.tables.get(&table).and_then(|maker| maker.sibling(id, totals)).ok_or_else(|| unheld("a table's"))?;
            slice_of(r, source)?
        }
        SOURCE => {
            let source = held.others.get(&r.u64()?).cloned().ok_or_else(|| unheld("a temporary"))?;
            slice_of(r, source)?
        }
        REFERENCE => Part::Reference(r.u64()?),
        LOCATOR => Part::Locator(r.u32()?),
        _ => return Err(r.corrupt("no such part of a large object")),
    })
}

/// The slice of `source` whose range `r` reads.
fn slice_of(r: &mut Reader<'_>, source: Arc<dyn LobSource>) -> Result<Part> {
    Ok(Part::Slice(Slice { source, octet: r.u64()?, char: r.u64()?, octets: r.u64()?, chars: r.u64()? }))
}

/// The error of a spilled record naming `what` large object its spill does
/// not hold.
fn unheld(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] a spilled row names {what} large object its spill does not hold (RULES.md D38, D79)"))
}

/// `l`, a reference to the `n`th value of `named`, as that value; any
/// other as it is.
fn resolved(l: &Lob, named: &[Vec<Part>]) -> Result<Lob> {
    let [Part::Reference(n)] = l.parts() else { return Ok(l.clone()) };
    let parts = usize::try_from(*n)
        .ok()
        .and_then(|n| named.get(n))
        .ok_or_else(|| SqlError::new(Code::Corrupt, "[DESIGN] a spilled row names a large object it does not write (RULES.md D38)"))?;
    let totals = (l.octets(), l.chars());
    Ok(match parts.as_slice() {
        [Part::Reference(id)] => Lob::of_reference(l.kind(), *id, totals),
        [Part::Locator(locator)] => Lob::of_locator(l.kind(), *locator, totals),
        _ => Lob::of_parts(l.kind(), parts.clone()),
    })
}

/// `v`, inside `depth` values, each reference in it to a value of `named`
/// made that value.
fn attached(v: &Value, depth: usize, named: &[Vec<Part>]) -> Result<Value> {
    if let Value::Lob(l) = v {
        return Ok(Value::Lob(resolved(l, named)?));
    }
    if let Value::Xml(x) = v
        && let Some(l) = x.stored_lob()
    {
        return Ok(Value::Xml(XmlValue::stored(resolved(l, named)?)));
    }
    if depth < MAX_TYPE_NESTING {
        return rebuilt(v, depth, &mut |p, at| attached(p, at, named));
    }
    Ok(v.clone())
}

// Unit tests: values named by a spill's records, read back as written
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
