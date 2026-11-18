//! \[EXTENSION\] An index's description (RULES.md D34): its name, table,
//! key columns, method, whether unique and options, and each key part that
//! is an expression, as written, with its place in the key (RULES.md
//! D92). What is derived of its table, its collations, whether it
//! holds nulls and its expressions bound, is not written.

use super::{count, flag, get_all, options, position, positions, put_all, put_flag, put_name, put_options, put_positions, qualified};
use crate::bytes::{Put, Reader};
use crate::catalog::{EXPRESSION_PART, IndexDesc, KeyExpression};
use crate::error::Result;

/// `index` in bytes.
pub(crate) fn put_index(out: &mut Vec<u8>, index: &IndexDesc) {
    out.put_str(&index.name);
    put_name(out, &index.table);
    put_positions(out, index.columns.iter());
    out.put_str(&index.method);
    put_flag(out, index.unique);
    put_options(out, &index.options);
    put_all(out, index.expressions.iter(), |out, e| {
        out.put_u32(count(e.part));
        out.put_str(&e.text);
    });
}

/// The index `put_index` wrote, each key part that is an expression held
/// as `EXPRESSION_PART` among its columns.
pub(crate) fn index(r: &mut Reader<'_>) -> Result<IndexDesc> {
    let (name, table, mut columns, method, unique, options) = (r.str()?, qualified(r)?, positions(r)?, r.str()?, flag(r)?, options(r)?);
    let expressions = get_all(r, |r| {
        let part = position(r)?;
        Ok(KeyExpression::unbound(part, r.str()?))
    })?;
    for e in &expressions {
        if let Some(c) = columns.get_mut(e.part) {
            *c = EXPRESSION_PART;
        }
    }
    Ok(IndexDesc { name, table, columns, method, unique, options, collations: Vec::new(), nulls: false, expressions })
}
