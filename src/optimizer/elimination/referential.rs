//! \[DESIGN\] An inner join on a referential constraint removed (RULES.md
//! D375). A row of the referencing table whose referencing columns are all
//! not null passes the constraint only if a row of the referenced table
//! matches them all, and of referencing columns that are NOT NULL every
//! match type means that [ISO/IEC 9075-2:2023 §4.25.3.3]. The referenced
//! columns are a unique key of their table, not deferrable [§11.8 SR4,
//! SR5], so that row is the only one; the rule finds that key itself. A filter over a product of the referencing table's
//! operand and the referenced table's whole scan, whose conjuncts that
//! read the referenced table are exactly the constraint's equalities,
//! keeps each row of the other operands once: its projection that reads no
//! column of the referenced table is that of the product without it. The
//! constraint must not be deferrable, as a deferred constraint may be
//! violated until it is checked [§4.25.2], nor of a period, nor either
//! table one of a period, and each column of its keys of no collation of
//! its own. The other conjuncts are
//! evaluated on the same rows of the other operands: once each, where
//! they were once for each row of the referenced table, which has one
//! whenever they have any, so they raise as before.

use super::{never_null, uncollated, unique_within};
use crate::catalog::{Catalog, ForeignKey, QualifiedName, TableDesc};
use crate::logical::{Cond, Expr, LogicalPlan, and_all, cond_columns, expr_columns, map_cond_columns, map_expr_columns, split_and};
use crate::value::CompOp;

/// The projection `exprs` of the filter of `cond` over the chain of
/// products `chain`, without the referenced table an operand of it joins
/// on a referential constraint, if there is such a table as the module
/// says.
pub(super) fn eliminated(exprs: &[Expr], cond: &Cond, chain: &LogicalPlan, catalog: &dyn Catalog) -> Option<LogicalPlan> {
    if !matches!(chain, LogicalPlan::Product { .. }) {
        return None;
    }
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let mut operands = Vec::new();
    operands_of(chain, 0, (&mut operands, &mut conjuncts));
    let starts: Vec<usize> = operands.iter().scan(0_usize, |at, o| Some(std::mem::replace(at, at.saturating_add(o.width())))).collect();
    let mut read = Vec::new();
    for e in exprs {
        expr_columns(e, &mut read);
    }
    let taken = Chain { operands: &operands, starts: &starts, conjuncts: &conjuncts, read: &read };
    let at = (0..operands.len()).find(|&at| taken.removable(at, catalog))?;
    Some(taken.without(at, exprs))
}

/// A filter's chain of products taken apart: its operands, where their
/// columns begin, its condition's conjuncts, and the columns the
/// projection over it reads.
struct Chain<'a, 'p> {
    operands: &'a [&'p LogicalPlan],
    starts: &'a [usize],
    conjuncts: &'a [Cond],
    read: &'a [usize],
}

impl Chain<'_, '_> {
    /// The columns of operand `at`.
    fn range(&self, at: usize) -> std::ops::Range<usize> {
        let start = self.starts.get(at).copied().unwrap_or_default();
        start..start.saturating_add(self.operands.get(at).map_or(0, |o| o.width()))
    }

    /// Whether operand `at` is a referenced table's whole scan, unread by
    /// the projection, that the conjuncts reading it join to one other
    /// operand on a referential constraint as the module says.
    fn removable(&self, at: usize, catalog: &dyn Catalog) -> bool {
        let Some(LogicalPlan::Scan { table, located: false, .. }) = self.operands.get(at) else { return false };
        let own = self.range(at);
        if self.read.iter().any(|c| own.contains(c)) {
            return false;
        }
        let pairs = self.pairs(&own);
        let Some(pairs) = pairs else { return false };
        let Some(referenced) = catalog.table(table) else { return false };
        let Some((other, _)) = pairs.first() else { return false };
        let Some(child) = self.operand_of(*other) else { return false };
        let Some((name, start)) = self.table_read(child) else { return false };
        let Some(desc) = catalog.table(name) else { return false };
        let local: Vec<(usize, usize)> = pairs.iter().map(|&(c, p)| (c.saturating_sub(start), p.saturating_sub(own.start))).collect();
        desc.foreign_keys().any(|(fk_name, fk)| referenced_by((desc, fk_name, fk), (table, referenced), &local))
    }

    /// Of the conjuncts reading the columns `own`, each a column of another
    /// operand equal to one of `own`, those pairs, if every such conjunct
    /// is one and all are of one other operand.
    fn pairs(&self, own: &std::ops::Range<usize>) -> Option<Vec<(usize, usize)>> {
        let mut pairs = Vec::new();
        for c in self.conjuncts {
            let mut columns = Vec::new();
            cond_columns(c, &mut columns);
            if !columns.iter().any(|col| own.contains(col)) {
                continue;
            }
            let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
            let pair = if own.contains(a) { (*b, *a) } else { (*a, *b) };
            if own.contains(&pair.0) || !own.contains(&pair.1) {
                return None;
            }
            pairs.push(pair);
        }
        let first = self.operand_of(pairs.first()?.0)?;
        pairs.iter().all(|(c, _)| self.operand_of(*c) == Some(first)).then_some(pairs)
    }

    /// The operand whose column `column` is.
    fn operand_of(&self, column: usize) -> Option<usize> {
        (0..self.operands.len()).rev().find(|&at| self.range(at).contains(&column))
    }

    /// The table operand `at` reads, and its first column, if it is a scan
    /// of it, which gives no row ids, or a filter of one.
    fn table_read(&self, at: usize) -> Option<(&QualifiedName, usize)> {
        let start = self.starts.get(at).copied()?;
        let operand = *self.operands.get(at)?;
        let scan = if let LogicalPlan::Filter { input, .. } = operand { &**input } else { operand };
        let LogicalPlan::Scan { table, located: false, .. } = scan else { return None };
        Some((table, start))
    }

    /// The projection `exprs` of the chain without operand `at`, the
    /// conjuncts that read it dropped.
    fn without(&self, at: usize, exprs: &[Expr]) -> LogicalPlan {
        let own = self.range(at);
        let width = own.len();
        let shift = |c: usize| if c >= own.end { c.saturating_sub(width) } else { c };
        let kept: Vec<LogicalPlan> = self.operands.iter().enumerate().filter(|(n, _)| *n != at).map(|(_, o)| (*o).clone()).collect();
        let mut rest = kept.into_iter();
        let first = rest.next().unwrap_or(LogicalPlan::Values { rows: vec![Vec::new()], types: Vec::new() });
        let product = rest.fold(first, |l, r| LogicalPlan::Product { left: Box::new(l), right: Box::new(r) });
        let conds = self.conjuncts.iter().filter(|c| !reads_any(c, &own)).map(|c| map_cond_columns(c.clone(), &shift)).collect();
        let input = match and_all(conds) {
            Some(cond) => LogicalPlan::Filter { cond, input: Box::new(product) },
            None => product,
        };
        let exprs = exprs.iter().map(|e| map_expr_columns(e.clone(), &shift)).collect();
        LogicalPlan::Project { exprs, input: Box::new(input) }
    }
}

/// Whether `c` reads a column of `own`.
fn reads_any(c: &Cond, own: &std::ops::Range<usize>) -> bool {
    let mut columns = Vec::new();
    cond_columns(c, &mut columns);
    columns.iter().any(|col| own.contains(col))
}

/// Whether the referential constraint `fk` of the referencing table
/// `desc` references the table `referenced` on exactly the pairs of a
/// referencing and a referenced column `pairs`, as the module says.
fn referenced_by(
    (desc, name, fk): (&TableDesc, &QualifiedName, &ForeignKey),
    (table, referenced): (&QualifiedName, &TableDesc),
    pairs: &[(usize, usize)],
) -> bool {
    let immediate = desc.constraints.iter().any(|c| &c.name == name && !c.attributes.deferrable);
    let plain = fk.period.is_none() && fk.columns.len() == pairs.len() && &fk.table == table;
    let matched = fk.columns.iter().zip(&fk.referenced).all(|(c, r)| pairs.contains(&(*c, *r)));
    let columns = fk.columns.iter().all(|&c| never_null(desc, c) && uncollated(desc, c));
    let keyed = fk.referenced.iter().all(|&r| uncollated(referenced, r)) && unique_within(referenced, &fk.referenced);
    let periods = desc.periods.is_empty() && referenced.periods.is_empty();
    immediate && plain && matched && columns && keyed && periods
}

/// The operands of the chain of products `plan`, its columns beginning at
/// `start`, left to right, the filters within it that cannot raise taken
/// apart into `conjuncts` over the chain's columns: a conjunction is
/// true of a row of the product exactly when each conjunct is, wherever
/// it is evaluated, and one that cannot raise may be evaluated of more
/// rows (RULES.md D15).
fn operands_of<'p>(plan: &'p LogicalPlan, start: usize, (out, conjuncts): (&mut Vec<&'p LogicalPlan>, &mut Vec<Cond>)) {
    if let LogicalPlan::Product { left, right } = plan {
        operands_of(left, start, (&mut *out, &mut *conjuncts));
        operands_of(right, start.saturating_add(left.width()), (out, conjuncts));
        return;
    }
    if let LogicalPlan::Filter { cond, input } = plan
        && matches!(**input, LogicalPlan::Product { .. })
        && !cond.can_raise()
    {
        split_and(map_cond_columns(cond.clone(), &|c| c.saturating_add(start)), conjuncts);
        operands_of(input, start, (out, conjuncts));
        return;
    }
    out.push(plan);
}
