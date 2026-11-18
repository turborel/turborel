//! \[EXTENSION\] The key parts of an index that are expressions (RULES.md
//! D92), as the index is defined and whenever it enters a
//! catalog: their places among its columns, and each bound over its
//! table's row, of the table as it is then.

use crate::binder::bind_key;
use crate::catalog::{EXPRESSION_PART, IndexDesc, KeyExpression, TableDesc};
use crate::db::{State, api};
use crate::error::Result;

/// The positions of the key parts of an index on `desc`: of each column
/// of `columns`, in order, and `EXPRESSION_PART` at the place of each
/// expression of `expressions`.
///
/// # Errors
///
/// SQLCODE -910 if the table has no such column, or a place is past the
/// key's end.
pub(in crate::db) fn key_positions(desc: &TableDesc, columns: &[String], expressions: &[(usize, String)]) -> Result<Vec<usize>> {
    let width = columns.len().saturating_add(expressions.len());
    let mut named = columns.iter();
    let mut out = Vec::with_capacity(width);
    for at in 0..width {
        if expressions.iter().any(|(part, _)| *part == at) {
            out.push(EXPRESSION_PART);
            continue;
        }
        let c = named.next().ok_or_else(|| api(format!("[EXTENSION] a key part of an index on {} is past its end", desc.name)))?;
        out.push(desc.column(c).ok_or_else(|| api(format!("[EXTENSION] {} has no column {c}", desc.name)))?);
    }
    Ok(out)
}

/// The key parts `expressions`, each its place and text, not yet bound.
pub(in crate::db) fn unbound_keys(expressions: &[(usize, String)]) -> Vec<KeyExpression> {
    expressions.iter().map(|(at, text)| KeyExpression::unbound(*at, text.clone())).collect()
}

impl State {
    /// `ix`, an index on `table`, its expressions bound over the table's
    /// row as it is now.
    ///
    /// # Errors
    ///
    /// As `binder::bind_key`: an expression that no longer binds, or that
    /// a change of its columns' types lets raise.
    pub(crate) fn bound_keys(&self, mut ix: IndexDesc, table: &TableDesc) -> Result<IndexDesc> {
        for e in &mut ix.expressions {
            e.bound = Some(bind_key(self, table, (&ix.name, &e.text))?);
        }
        Ok(ix)
    }
}

impl KeyExpression {
    /// Whether `ix`'s key parts that are expressions read no column `gone`
    /// says is gone.
    pub(in crate::db) fn kept(ix: &IndexDesc, gone: impl Fn(usize) -> bool) -> bool {
        ix.expressions.iter().filter_map(|e| e.bound.as_ref()).all(|b| !b.reads().iter().any(|&c| gone(c)))
    }
}
