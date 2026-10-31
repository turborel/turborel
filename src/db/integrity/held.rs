//! \[DESIGN\] What a data-changing statement did, and the checks of its
//! constraints, as records of a list that spills (RULES.md D38,
//! `exec/held.rs`): held up to MEMORY.WORK_BYTES, then written to runs of a
//! spill area, for the checks at its end, and those of deferred
//! constraints until they are made. Split from db/integrity.rs (RULES.md
//! R-SIZE-2).
//!
//! ```text
//! name:   its schema's name, then its own, each a u32 length and UTF-8
//! effect: 0 inserted, 1 updated, 2 deleted (u8); its table's name; its
//!         row's id u64; then, updated or deleted, the old row; then,
//!         updated, the columns given values, a u32 count and each u32
//! check:  0 row, 1 unique (u8), the table's and the constraint's names,
//!         the row's id u64; 2 table, the two names; 3 referenced, the
//!         names, the old row, 0 or 1 and the row's id u64 (u8); 4
//!         assertion, its name; 5 asserted row, the table's and the
//!         assertion's names, the row's id u64
//! ```

use super::{Check, Effect};
use crate::bytes::{Put, Reader};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::exec::Holdable;
use crate::spill::{ListItem, row_bytes};
use crate::storage::RowId;
use crate::value::codec::{put_row, row};
use crate::value::types::TypeRegistry;

/// The kinds of effect, as a record holds them (above).
const INSERTED: u8 = 0;
const UPDATED: u8 = 1;
const DELETED: u8 = 2;
/// The kinds of check, as a record holds them (above).
const ROW: u8 = 0;
const UNIQUE: u8 = 1;
const TABLE: u8 = 2;
const REFERENCED: u8 = 3;
const ASSERTION: u8 = 4;
const ASSERTED_ROW: u8 = 5;

/// The bytes a name holds in memory, near enough.
fn name_bytes(name: &QualifiedName) -> usize {
    size_of::<QualifiedName>().saturating_add(name.auth.len()).saturating_add(name.ident.len())
}

fn put_name(out: &mut Vec<u8>, name: &QualifiedName) {
    out.put_str(&name.auth);
    out.put_str(&name.ident);
}

fn name(r: &mut Reader<'_>) -> Result<QualifiedName> {
    Ok(QualifiedName { auth: r.str()?, ident: r.str()? })
}

/// A count or a column, as a record holds it.
fn put_usize(out: &mut Vec<u8>, n: usize) {
    out.put_u32(u32::try_from(n).unwrap_or(u32::MAX));
}

fn usize_of(r: &mut Reader<'_>) -> Result<usize> {
    usize::try_from(r.u32()?).map_err(|_| r.corrupt("a number past the machine's"))
}

impl ListItem for Effect {
    fn bytes(&self) -> usize {
        let of = |table: &QualifiedName| size_of::<Effect>().saturating_add(name_bytes(table));
        match self {
            Effect::Inserted { table, .. } => of(table),
            Effect::Updated { table, old, columns, .. } => {
                of(table).saturating_add(row_bytes(old)).saturating_add(columns.len().saturating_mul(size_of::<usize>()))
            }
            Effect::Deleted { table, old, .. } => of(table).saturating_add(row_bytes(old)),
        }
    }
}

impl Holdable for Effect {
    fn record(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Effect::Inserted { table, id } => {
                out.put_u8(INSERTED);
                put_name(&mut out, table);
                out.put_u64(*id);
            }
            Effect::Updated { table, id, old, columns } => {
                out.put_u8(UPDATED);
                put_name(&mut out, table);
                out.put_u64(*id);
                put_row(&mut out, old);
                put_usize(&mut out, columns.len());
                for &c in columns {
                    put_usize(&mut out, c);
                }
            }
            Effect::Deleted { table, id, old } => {
                out.put_u8(DELETED);
                put_name(&mut out, table);
                out.put_u64(*id);
                put_row(&mut out, old);
            }
        }
        out
    }

    fn read(record: &[u8], types: &TypeRegistry) -> Result<Effect> {
        let mut r = Reader::new(record, "held effect").with_types(types);
        let (kind, table, id) = (r.u8()?, name(&mut r)?, r.u64()?);
        let effect = match kind {
            INSERTED => Effect::Inserted { table, id },
            UPDATED => {
                let old = row(&mut r)?;
                let columns = (0..usize_of(&mut r)?).map(|_| usize_of(&mut r)).collect::<Result<_>>()?;
                Effect::Updated { table, id, old, columns }
            }
            DELETED => Effect::Deleted { table, id, old: row(&mut r)? },
            _ => return Err(r.corrupt("no such effect")),
        };
        r.finish()?;
        Ok(effect)
    }
}

impl ListItem for Check {
    fn bytes(&self) -> usize {
        let table = if let Check::AssertedRow { table, .. } = self { Some(table) } else { self.table() };
        let names = name_bytes(self.constraint()).saturating_add(table.map_or(0, name_bytes));
        let old = if let Check::Referenced { old, .. } = self { row_bytes(old) } else { 0 };
        size_of::<Check>().saturating_add(names).saturating_add(old)
    }
}

impl Holdable for Check {
    fn record(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Check::Row { table, constraint, id } | Check::Unique { table, constraint, id } => {
                out.put_u8(if matches!(self, Check::Row { .. }) { ROW } else { UNIQUE });
                put_name(&mut out, table);
                put_name(&mut out, constraint);
                out.put_u64(*id);
            }
            Check::Table { table, constraint } => {
                out.put_u8(TABLE);
                put_name(&mut out, table);
                put_name(&mut out, constraint);
            }
            Check::Referenced { child, constraint, old, id } => {
                out.put_u8(REFERENCED);
                put_name(&mut out, child);
                put_name(&mut out, constraint);
                put_row(&mut out, old);
                out.put_u8(u8::from(id.is_some()));
                out.put_u64(id.unwrap_or_default());
            }
            Check::Assertion { constraint } => {
                out.put_u8(ASSERTION);
                put_name(&mut out, constraint);
            }
            Check::AssertedRow { table, constraint, id } => {
                out.put_u8(ASSERTED_ROW);
                put_name(&mut out, table);
                put_name(&mut out, constraint);
                out.put_u64(*id);
            }
        }
        out
    }

    fn read(record: &[u8], types: &TypeRegistry) -> Result<Check> {
        let mut r = Reader::new(record, "held check").with_types(types);
        let kind = r.u8()?;
        let check = if kind == ASSERTION { Check::Assertion { constraint: name(&mut r)? } } else { of_table(kind, &mut r)? };
        r.finish()?;
        Ok(check)
    }
}

/// The check of kind `kind`, of a table's constraint, read by `r` after
/// its kind.
fn of_table(kind: u8, r: &mut Reader<'_>) -> Result<Check> {
    let (table, constraint) = (name(r)?, name(r)?);
    Ok(match kind {
        ROW => Check::Row { table, constraint, id: r.u64()? },
        UNIQUE => Check::Unique { table, constraint, id: r.u64()? },
        ASSERTED_ROW => Check::AssertedRow { table, constraint, id: r.u64()? },
        TABLE => Check::Table { table, constraint },
        REFERENCED => {
            let old = row(r)?;
            let (some, id): (u8, RowId) = (r.u8()?, r.u64()?);
            Check::Referenced { child: table, constraint, old, id: (some == 1).then_some(id) }
        }
        _ => return Err(r.corrupt("no such check")),
    })
}

// Unit tests: each effect and check read back as written (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
