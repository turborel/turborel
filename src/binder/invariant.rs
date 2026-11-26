//! [ISO/IEC 9075-2:2023 §6.7 SR7, SR8, GR1e] A column reference of a
//! group: a grouping column's, the group's value of it; or a group-invariant
//! column reference, whose column is functionally dependent on the
//! grouping columns of each grouping set [§4.26], its value that of the
//! group's first row, a set function the grouping computes with the others
//! (`functions/any_value.rs`). The functional dependencies are those of the
//! query's FROM and WHERE (`binder/dependencies.rs`), found the first time
//! a column is not a grouping column.

use std::sync::Arc;

use super::dependencies::Known;
use super::from::{Tree, from_plan};
use super::{Binder, Grouping, Scope, nondeterministic, rule};
use crate::ast::{ColumnSpecification, ValueExpression};
use crate::error::Result;
use crate::functions::{AggregateRef, GroupValue, RowOf, ScalarRef};
use crate::logical::{AggCall, Cond, Expr, LogicalPlan};
use crate::value::DataType;
use crate::value::collation::CollationRef;
use crate::value::types::Field;

/// The FROM and WHERE of a query, kept while its select list, HAVING and
/// window clause are bound, and what is known of their rows once found.
#[derive(Default)]
pub(super) struct Rows {
    trees: Vec<Tree>,
    cond: Option<Cond>,
    known: Option<Known>,
}

impl Binder<'_> {
    /// Keeps the bound FROM `trees` and WHERE `cond` of its query, whose
    /// grouped clauses are bound next.
    pub(super) fn keep_rows(&self, trees: Vec<Tree>, cond: Option<Cond>) {
        self.rows.replace(Rows { trees, cond, known: None });
    }

    /// The FROM and WHERE `keep_rows` kept, given back to be planned.
    pub(super) fn kept_rows(&self) -> (Vec<Tree>, Option<Cond>) {
        let Rows { trees, cond, .. } = self.rows.take();
        (trees, cond)
    }

    /// FROM-row column `pos`, referenced by `cs`, of type `t`, in the rows
    /// `scope` sees. In a group it is a grouping column, its value of the
    /// group, which of grouping sets is null in a set that leaves it out
    /// [§7.13 SR17]; or one functionally dependent on them (`invariant`).
    /// In a constraint, such a column of a character string is possibly
    /// non-deterministic [ISO/IEC 9075:1992 §11.9 SR6, §7.8 SR3b, §7.9
    /// SR13c ii].
    pub(super) fn column_in(&self, cs: &ColumnSpecification, (pos, t): (usize, &DataType), scope: Scope<'_>) -> Result<Expr> {
        if let Some(graph) = self.graph_column(pos, scope) {
            return Ok(graph);
        }
        if let Some(pattern) = self.pattern_column(pos, scope)? {
            return Ok(pattern);
        }
        let Scope::Groups(g, why) = scope else { return Ok(Expr::Column(pos)) };
        let found = match &g.sets {
            Some(sets) => sets.value_of(pos).cloned(),
            None => g.columns.iter().position(|&c| c == pos).map(Expr::Column),
        };
        let Some(e) = found else { return self.invariant(cs, (pos, t), (g, why)) };
        if self.purpose.constrains() && t.is_character() {
            return Err(nondeterministic("§7.8 SR3b, §7.9 SR13c ii", "a reference to a character string grouping column"));
        }
        Ok(e)
    }

    /// [§6.7 SR8] The FROM-row column `pos`, referenced by `cs`, of type
    /// `t`, no grouping column of `g`: functionally dependent on the
    /// grouping columns of each of its grouping sets, else the error `why`
    /// says. Its value is the group's first row's [GR1e], which of a
    /// character string, a datetime with time zone or a user-defined type
    /// is implementation-dependent (UV070), so in a constraint possibly
    /// non-deterministic.
    fn invariant(&self, cs: &ColumnSpecification, (pos, t): (usize, &DataType), (g, why): (&Grouping, &str)) -> Result<Expr> {
        if !self.dependent(pos, &g.determinants)? {
            return Err(rule(format!("{why} (found {})", self.names.get(pos).map_or("", String::as_str))));
        }
        if self.purpose.constrains() && chosen_of_group(t) {
            return Err(rule(
                "[ISO/IEC 9075:1992 §11.9 SR6; ISO/IEC 9075-2:2023 §6.7 GR1e] a CHECK condition shall not contain a query that is possibly non-deterministic: a reference to a column functionally dependent on grouping columns, of a character string, a datetime with time zone or a user-defined type",
            ));
        }
        let collation = if t.is_character() { self.chosen(None, self.derived(&ValueExpression::Column(cs.clone()))?)? } else { None };
        Ok(carried(g, (pos, t), self.names.get(pos).cloned().unwrap_or_default(), collation))
    }

    /// [§6.7 SR8, §4.26.2] Whether the FROM-row column `pos` is
    /// functionally dependent on the columns of each of `sets` in the rows
    /// of its query's FROM and WHERE: found once, of their plan.
    fn dependent(&self, pos: usize, sets: &[Vec<usize>]) -> Result<bool> {
        // Taken out while found, so no borrow is held as plans are made,
        // and put back whatever is found.
        let mut rows = self.rows.take();
        let found = match rows.known.take() {
            Some(known) => Ok(known),
            None => self.known_of(&rows),
        };
        let dependent = found.map(|known| {
            let dependent = !sets.is_empty() && sets.iter().all(|s| known.determines(s, pos));
            rows.known = Some(known);
            dependent
        });
        self.rows.replace(rows);
        dependent
    }

    /// What is known of the rows of the FROM and WHERE `rows`.
    fn known_of(&self, rows: &Rows) -> Result<Known> {
        let mut plan = from_plan(self, &rows.trees)?;
        if let Some(cond) = rows.cond.clone() {
            plan = LogicalPlan::Filter { cond, input: Box::new(plan) };
        }
        Ok(Known::of(self.db, &plan))
    }
}

impl Grouping {
    /// Whether the FROM-row column `pos` is a grouping column of it.
    pub(super) fn groups_by(&self, pos: usize) -> bool {
        match &self.sets {
            Some(sets) => sets.value_of(pos).is_some(),
            None => self.columns.contains(&pos),
        }
    }

    /// [§7.13 GR2a] Whether a group of it may have no rows: one of no
    /// GROUP BY, of GROUP BY (), or of an empty grouping set, in which a
    /// group-invariant column's value is null.
    pub(super) fn may_be_empty(&self) -> bool {
        match &self.sets {
            Some(sets) => sets.has_empty(),
            None => self.columns.is_empty(),
        }
    }
}

/// [§6.7 GR1e] Whether the value a group has of a column of type `t` is
/// chosen of its rows' (UV070): of a character string, a datetime with time
/// zone or a user-defined type.
fn chosen_of_group(t: &DataType) -> bool {
    let zoned = matches!(t, DataType::Time { zoned: true, .. } | DataType::Timestamp { zoned: true, .. });
    zoned || t.is_character() || matches!(t, DataType::Distinct(_) | DataType::Extension(_))
}

/// Whether the set function `a` keeps the value of the FROM-row column
/// `pos`: its argument is a row of that column alone.
fn carries(a: &AggCall, pos: usize) -> bool {
    let Some(Expr::Call(_, args)) = &a.arg else { return false };
    if let [Expr::Column(c)] = args.as_slice() { *c == pos } else { false }
}

/// [§6.7 GR1e] The value of the group-invariant column `pos`, of type `t`
/// and display name `name`, in a group of `g`: the column of the set
/// function that keeps it, added once, compared by `collation`, as a
/// grouping column of it would be.
fn carried(g: &Grouping, (pos, t): (usize, &DataType), name: String, collation: Option<CollationRef>) -> Expr {
    let mut aggs = g.aggs.borrow_mut();
    let k = if let Some(k) = aggs.iter().position(|a| a.label == name && carries(a, pos)) {
        k
    } else {
        aggs.push(group_value(pos, t, name));
        g.collated.borrow_mut().push(collation);
        aggs.len().saturating_sub(1)
    };
    Expr::Column(g.columns.len().saturating_add(k))
}

/// [§6.7 GR1e] The set function that keeps a group's value of its input's
/// column `pos`, of type `t` and display name `name`, which is
/// functionally dependent on the grouping columns: the column's in the
/// group's first row, a null too (`functions/any_value.rs`).
pub(crate) fn group_value(pos: usize, t: &DataType, name: String) -> AggCall {
    let row = DataType::row(vec![Field { name: name.clone(), data_type: t.clone(), unnamed: false }]);
    let arg = Expr::Call(ScalarRef(Arc::new(RowOf(row))), vec![Expr::Column(pos)]);
    let func = AggregateRef(Arc::new(GroupValue));
    AggCall { func, arg: Some(arg), distinct: false, filter: None, distribution: None, result: t.clone(), label: name }
}
