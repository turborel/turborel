//! \[EXTENSION\] How the planner reads an index whose key parts are
//! expressions (RULES.md D92). Each distinct expression of a
//! table's indexes is planned as a column past the table's own, of its
//! declared type, nullable unless it is never null: an index's key part is that column, and a
//! query's expression of the same canonical form (`index::expression`)
//! is a reference to it. The access paths then find, order and price the
//! index as one on columns. Only the choice of path sees these columns:
//! the filter a scan keeps, and the conjuncts an index scan shows it
//! answers, are the query's own. A key part raises nothing of any row, so
//! a conjunct of one is no conjunct that can raise (RULES.md D15).

use std::borrow::Cow;
use std::sync::Arc;

use super::shown::Shown;
use super::{PhysicalPlanner, TableRead};
use crate::catalog::{ColumnDesc, IndexDesc, QualifiedName, TableDesc};
use crate::db::State;
use crate::index::expression::canonical;
use crate::logical::{Cond, Expr, LogicalPlan};
use crate::storage::TableStorage;
use crate::value::DataType;

/// A distinct expression of a table's indexes: its canonical form, its
/// text as written, its type and whether it is never null.
struct Part {
    canonical: String,
    text: String,
    data_type: DataType,
    not_null: bool,
}

/// The expressions of a table's indexes, each planned as the column at
/// the table's width and after, in the order the catalog gives them; and
/// the names an access path's description is made of, those of the
/// table's columns, then each expression's text, if it is shown.
pub(super) struct Keyed {
    width: usize,
    parts: Vec<Part>,
    names: Option<Vec<String>>,
}

impl Keyed {
    /// Of the indexes of `table` in `db`'s catalog.
    pub(super) fn of(db: &State, table: &QualifiedName) -> Keyed {
        let width = db.catalog.table(table).map_or(0, |t| t.columns.len());
        let mut parts: Vec<Part> = Vec::new();
        for ix in db.catalog.indexes(table) {
            for (e, bound) in ix.expressions.iter().filter_map(|e| Some((e, e.bound.as_ref()?))) {
                if !parts.iter().any(|p| p.canonical == bound.canonical()) {
                    let (canonical, text, data_type) = (bound.canonical().to_owned(), e.text.clone(), bound.data_type().clone());
                    parts.push(Part { canonical, text, data_type, not_null: bound.not_null() });
                }
            }
        }
        Keyed { width, parts, names: None }
    }

    /// Of the table `read` reads, its description's names extended.
    pub(super) fn of_read(db: &State, read: TableRead<'_>) -> Keyed {
        let keyed = Keyed::of(db, read.table);
        let names = read.shown.names().filter(|_| !keyed.parts.is_empty()).map(|names| {
            let texts = keyed.parts.iter().map(|p| format!("({})", p.text));
            names.iter().cloned().chain(texts).collect()
        });
        Keyed { names, ..keyed }
    }

    /// `read`, its description made of the names extended, if any.
    pub(super) fn read<'k>(&'k self, read: TableRead<'k>) -> TableRead<'k> {
        match &self.names {
            Some(names) => TableRead { shown: Shown::of(Some(names)), ..read },
            None => read,
        }
    }

    /// The column an expression of `e`'s canonical form is planned as.
    pub(super) fn column_of(&self, e: &Expr) -> Option<usize> {
        if self.parts.is_empty() || matches!(e, Expr::Column(_) | Expr::Const(_)) {
            return None;
        }
        let form = canonical(e);
        let at = self.parts.iter().position(|p| p.canonical == form)?;
        self.width.checked_add(at)
    }

    /// `conds`, each expression of a key part's form in them a reference
    /// to its column.
    pub(super) fn substituted<'c>(&self, conds: &'c [Cond]) -> Cow<'c, [Cond]> {
        if self.parts.is_empty() {
            return Cow::Borrowed(conds);
        }
        Cow::Owned(conds.iter().map(|c| c.clone().map_exprs(&mut |e| self.replaced(e))).collect())
    }

    /// `e`, or a reference to the column of its form, or of the forms of
    /// the expressions it is made of. A bound expression is nested no
    /// deeper than the binder lets it, as every walk of one is.
    fn replaced(&self, e: Expr) -> Expr {
        match self.column_of(&e) {
            Some(at) => Expr::Column(at),
            None => e.map_children(&mut |child| self.replaced(child)),
        }
    }

    /// `desc` with a column of each expression, and `ix`'s key part of
    /// each its column.
    fn planned<'a>(&self, desc: &'a TableDesc, ix: &'a IndexDesc) -> Cow<'a, IndexDesc> {
        if ix.expressions.is_empty() {
            return Cow::Borrowed(ix);
        }
        let mut planned = ix.clone();
        for e in &ix.expressions {
            let at = e.bound.as_ref().and_then(|b| self.parts.iter().position(|p| p.canonical == b.canonical()));
            if let (Some(c), Some(at)) = (planned.columns.get_mut(e.part), at) {
                *c = desc.columns.len().saturating_add(at);
            }
        }
        Cow::Owned(planned)
    }
}

impl PhysicalPlanner<'_> {
    /// The storage of `table`, and the table as the access paths plan it
    /// (`planned`); none if either is missing.
    pub(super) fn planned(&self, table: &QualifiedName) -> Option<(&Arc<dyn TableStorage>, Planned<'_>)> {
        Some((self.db.storage_of(table).ok()?, planned(self.db, table)?))
    }
}

/// A table as the access paths plan it, its indexes, and its expressions'
/// columns.
pub(super) type Planned<'a> = (Cow<'a, TableDesc>, Vec<Cow<'a, IndexDesc>>, Keyed);

/// The table `table` of `db` as the access paths plan it, a column after
/// its own for each expression of its indexes, and its indexes, each key
/// part that is an expression on its column, and the columns planned;
/// none if there is no table.
fn planned<'a>(db: &'a State, table: &QualifiedName) -> Option<Planned<'a>> {
    let desc = db.catalog.table(table)?;
    let keyed = Keyed::of(db, table);
    // [EXTENSION] A global index is a path of the partitioned table's read
    // of its partitions' rows (`access/partitioned.rs`), not of its own.
    let own = db.catalog.indexes(table).into_iter().filter(|ix| !crate::catalog::is_global_index(&ix.name));
    let indexes = own.map(|ix| keyed.planned(desc, ix)).collect();
    if keyed.parts.is_empty() {
        return Some((Cow::Borrowed(desc), indexes, keyed));
    }
    let mut wide = desc.clone();
    wide.columns.extend(keyed.parts.iter().map(|p| ColumnDesc {
        name: format!("({})", p.text),
        data_type: p.data_type.clone(),
        not_null: p.not_null,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    }));
    Some((Cow::Owned(wide), indexes, keyed))
}

/// The column of `input`'s rows whose order gives the order of `e`: the
/// column `e` is, or, over a scan of a table, possibly filtered, the column
/// planned of an index's key part of `e`'s form (`Keyed`).
pub(super) fn ordered_by(db: &State, e: &Expr, input: &LogicalPlan) -> Option<usize> {
    if let Expr::Column(c) = e {
        return Some(*c);
    }
    Keyed::of(db, scanned(input)?).column_of(e)
}

/// The table `plan` scans, under filters, if it is a scan.
fn scanned(plan: &LogicalPlan) -> Option<&QualifiedName> {
    match plan {
        LogicalPlan::Scan { table, .. } => Some(table),
        LogicalPlan::Filter { input, .. } => scanned(input),
        LogicalPlan::Product { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::SemiJoin { .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Project { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(_)
        | LogicalPlan::Window(_)
        | LogicalPlan::Recognize(_)
        | LogicalPlan::Polymorphic(_) => None,
    }
}
