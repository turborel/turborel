//! \[DESIGN\] The keys a join's estimate reads (RULES.md D384): of each
//! table, its unique keys and its referential constraints, from the
//! catalog, so they are known before any statistics are.
//!
//! A unique key is the columns of a UNIQUE or PRIMARY KEY constraint not
//! deferrable and of no period, or of a unique index of columns. No two
//! rows are equal in all of them, nulls aside, and an equality is never
//! true of a null [X3.135-1986 §5.11 GR2], so a probe of all of them by
//! equalities finds at most one row. A deferred constraint may be broken
//! until it is checked [ISO/IEC 9075-2:2023 §4.25.2], and a period's key
//! is unique only of rows whose periods overlap, so neither is one.
//!
//! A referential constraint not deferrable and of no period whose
//! referencing columns are NOT NULL gives each referencing row exactly
//! one referenced row [§4.25.3.3, §11.8 GR2].
//!
//! A plan's columns hold a key of it when they hold one of the table it
//! reads, through filters, sorts, limits and projections of columns, or
//! all of a grouping's grouping columns, or all of DISTINCT's columns,
//! whose rows are each of a value of their own.

use std::rc::Rc;

use super::{Estimates, MAX_ESTIMATE_DEPTH};
use crate::catalog::{ConstraintKind, QualifiedName, TableDesc};
use crate::logical::{Expr, LogicalPlan};

/// A table's keys, as the catalog describes them.
#[derive(Debug, Default)]
pub(in crate::physical) struct TableKeys {
    /// Each unique key's columns.
    unique: Vec<Vec<usize>>,
    /// Each referential constraint that gives each row one referenced
    /// row: its referencing columns, the referenced table and columns.
    references: Vec<(Vec<usize>, QualifiedName, Vec<usize>)>,
}

impl TableKeys {
    /// The keys of the table `desc`, its unique indexes' keys `indexed`
    /// among them.
    fn of(desc: &TableDesc, indexed: impl Iterator<Item = Vec<usize>>) -> TableKeys {
        let immediate = |name: &QualifiedName| desc.constraints.iter().any(|c| &c.name == name && !c.attributes.deferrable);
        let mut unique: Vec<Vec<usize>> = indexed.collect();
        for c in desc.constraints.iter().filter(|c| !c.attributes.deferrable) {
            let constrained = match &c.kind {
                ConstraintKind::Unique { columns, period: None, .. } => Some(columns.clone()),
                ConstraintKind::Unique { period: Some(_), .. }
                | ConstraintKind::NotNull(_)
                | ConstraintKind::ForeignKey(_)
                | ConstraintKind::Check { .. }
                | ConstraintKind::Connection(_) => None,
            };
            unique.extend(constrained);
        }
        let not_null = |columns: &[usize]| columns.iter().all(|&c| desc.columns.get(c).is_some_and(|d| d.not_null));
        let references = desc
            .foreign_keys()
            .filter(|(name, fk)| immediate(name) && fk.period.is_none() && not_null(&fk.columns))
            .map(|(_, fk)| (fk.columns.clone(), fk.table.clone(), fk.referenced.clone()))
            .collect();
        TableKeys { unique, references }
    }

    /// Whether `columns` hold every column of one of its unique keys.
    pub(in crate::physical) fn covers(&self, columns: &[usize]) -> bool {
        self.unique.iter().any(|key| !key.is_empty() && key.iter().all(|k| columns.contains(k)))
    }

    /// Whether a referential constraint references `table` on pairs of a
    /// referencing and a referenced column all among `pairs`.
    pub(in crate::physical) fn references(&self, table: &QualifiedName, pairs: &[(usize, usize)]) -> bool {
        self.references.iter().any(|(columns, to, referenced)| {
            to == table && !columns.is_empty() && columns.iter().zip(referenced).all(|(c, r)| pairs.contains(&(*c, *r)))
        })
    }
}

impl Estimates<'_> {
    /// The keys of `table`, read of the catalog once this planning.
    pub(in crate::physical) fn table_keys(&self, table: &QualifiedName) -> Rc<TableKeys> {
        let known = self.table_keys.borrow().get(table).cloned();
        if let Some(keys) = known {
            return keys;
        }
        let db = self.db();
        let indexed = db.catalog.indexes(table).into_iter().filter(|ix| ix.unique && ix.expressions.is_empty()).map(|ix| ix.columns.clone());
        let keys = Rc::new(db.catalog.table(table).map(|desc| TableKeys::of(desc, indexed)).unwrap_or_default());
        self.table_keys.borrow_mut().insert(table.clone(), Rc::clone(&keys));
        keys
    }

    /// The distinct values `plan`'s columns `columns` hold together, if
    /// they hold a key of it: of a table's key, the table's rows; of a
    /// grouping's or DISTINCT's, its estimated rows.
    pub(in crate::physical) fn key_of(&self, plan: &LogicalPlan, columns: &[usize]) -> Option<f64> {
        let (mut at, mut columns) = (plan, columns.to_vec());
        for _ in 0..MAX_ESTIMATE_DEPTH {
            match at {
                LogicalPlan::Scan { table, .. } => {
                    return self.table_keys(table).covers(&columns).then(|| self.table_rows(table)).flatten();
                }
                LogicalPlan::Filter { input, .. } | LogicalPlan::Sort { input, .. } | LogicalPlan::Limit { input, .. } => at = input,
                LogicalPlan::Project { exprs, input } => {
                    columns = columns.iter().map(|&c| if let Some(Expr::Column(i)) = exprs.get(c) { Some(*i) } else { None }).collect::<Option<_>>()?;
                    at = input;
                }
                LogicalPlan::Aggregate { groups, seeds, .. } => {
                    let all = !groups.is_empty() && seeds.is_empty() && (0..groups.len()).all(|g| columns.contains(&g));
                    return all.then(|| self.card_at(at, 1).rows);
                }
                LogicalPlan::Distinct { input } => return (0..input.width()).all(|c| columns.contains(&c)).then(|| self.card_at(at, 1).rows),
                LogicalPlan::Product { .. }
                | LogicalPlan::OuterJoin { .. }
                | LogicalPlan::SetOp { .. }
                | LogicalPlan::System { .. }
                | LogicalPlan::Values { .. }
                | LogicalPlan::Recursive { .. }
                | LogicalPlan::WorkTable { .. }
                | LogicalPlan::Transition { .. }
                | LogicalPlan::Unnest(..)
                | LogicalPlan::Window(..)
                | LogicalPlan::Recognize(..)
                | LogicalPlan::Polymorphic(..) => return None,
            }
        }
        None
    }
}
