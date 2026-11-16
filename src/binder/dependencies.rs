//! [ISO/IEC 9075-2:2023 §4.26] Known functional dependencies of the rows
//! of a bound plan: which sets of its columns determine which. A grouped
//! query's group-invariant column references need them of its FROM and
//! WHERE [§6.7 SR8] (`binder/invariant.rs`).
//!
//! They are found over the plan, node by node, by the rules the standard
//! gives for the syntax each node is made of [§4.26.3 to §4.26.16]: a
//! base table's keys and generated columns (`tables.rs`), the equalities
//! of a condition and the outer joins (`conditions.rs`), the rest here.
//! Each column is named by an attribute, which a projection, a join or a
//! grouping that keeps the column keeps. So a dependency found below holds
//! above, through columns a projection drops too, as the rules of
//! deduction give [§4.26.1]. A node whose rows are made anew, a UNION's or
//! a system table's, has new attributes.
//!
//! Every rule gives only dependencies that hold. Beyond the standard's
//! (IE015) it gives those of a table value constructor of one row, of the
//! deterministic expressions an equality compares a column with [§4.26.12
//! notes 68, 69], of a UNIQUE NULLS NOT DISTINCT of nullable columns, of
//! the ordinality column of a collection derived table that reads columns
//! before it, with them, and of a COALESCE of a column known not null.

mod conditions;
mod keys;
mod tables;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::collections::BTreeSet;

use crate::db::State;
use crate::logical::{Cond, Expr, LogicalPlan, SetOp, Unnest, expr_columns};
use crate::value::collation::CollationRef;

pub(super) use conditions::deterministic;
pub(crate) use keys::{DescribedKey, described_key};

/// A set of attributes.
type Attrs = BTreeSet<usize>;

/// What is known of the rows of a plan [§4.26.2].
#[derive(Clone, Default)]
pub(super) struct Known {
    /// The attribute of each of its columns.
    columns: Vec<usize>,
    /// Its known functional dependencies, each A ↦ B, but the axiomatic.
    dependencies: Vec<(Attrs, Attrs)>,
    /// The attributes known not null.
    not_null: Attrs,
    /// Pairs (x, y) of attributes such that y is null wherever x is: of a
    /// LEFT join's equality of x, of its first operand, and y, of its
    /// second, which matches no row where x is null.
    nulled_with: Vec<(usize, usize)>,
    /// The collation of each attribute of a base table's column that has
    /// one: an equality by it is of the column's values.
    collations: Vec<(usize, CollationRef)>,
    /// Its BUC-sets, each with whether it is a BPK-set too [§4.26.2]. The
    /// deduction rule for them is applied as a candidate key is found
    /// (`keys.rs`).
    keys: Vec<(Attrs, bool)>,
    /// Of a product, the BUC-sets and BPK-sets of the table references it
    /// is of, which a condition over it makes its own where they determine
    /// every column [§4.26.11, §4.26.12].
    pending: Vec<(Attrs, bool)>,
    /// Of a grouped table, the attributes of its grouping columns [§4.27].
    grouping: Option<Attrs>,
}

/// A walk of a plan: the database it reads, and the next attribute not
/// yet given.
struct Walk<'a> {
    db: &'a State,
    next: Cell<usize>,
}

impl Walk<'_> {
    /// A new attribute.
    fn attribute(&self) -> usize {
        let n = self.next.get();
        self.next.set(n.saturating_add(1));
        n
    }

    /// Rows of `width` new columns, of which nothing is known.
    fn fresh(&self, width: usize) -> Known {
        Known { columns: (0..width).map(|_| self.attribute()).collect(), ..Known::default() }
    }
}

impl Known {
    /// What is known of the rows of `plan`, of the database `db`.
    pub(super) fn of(db: &State, plan: &LogicalPlan) -> Known {
        known(&Walk { db, next: Cell::new(0) }, plan)
    }

    /// [§4.26.1] Whether the columns `determinants` determine the column
    /// `c`: whether its attribute is in the closure of theirs.
    pub(super) fn determines(&self, determinants: &[usize], c: usize) -> bool {
        let start = determinants.iter().filter_map(|&d| self.attribute_of(d)).collect();
        self.attribute_of(c).is_some_and(|a| self.closure(start).contains(&a))
    }

    /// [§4.26.1] The attributes `start` determines: by the rules of
    /// deduction, each B of a dependency A ↦ B whose A it determines, until
    /// no more are added.
    fn closure(&self, mut start: Attrs) -> Attrs {
        loop {
            let before = start.len();
            for (a, b) in &self.dependencies {
                if a.is_subset(&start) {
                    start.extend(b.iter().copied());
                }
            }
            if start.len() == before {
                return start;
            }
        }
    }

    /// The attribute of its column `c`.
    fn attribute_of(&self, c: usize) -> Option<usize> {
        self.columns.get(c).copied()
    }

    /// The attributes of its columns `columns`, if it has them all.
    fn attributes_of(&self, columns: &[usize]) -> Option<Attrs> {
        columns.iter().map(|&c| self.attribute_of(c)).collect()
    }

    /// Adds A ↦ B, unless it is axiomatic, B a subset of A [§4.26.1].
    fn depends(&mut self, a: Attrs, b: Attrs) {
        if !b.is_subset(&a) {
            self.dependencies.push((a, b));
        }
    }

    /// Adds that the attributes `x` and `y` determine each other.
    fn same(&mut self, x: usize, y: usize) {
        self.depends(Attrs::from([x]), Attrs::from([y]));
        self.depends(Attrs::from([y]), Attrs::from([x]));
    }

    /// Adds that the columns an expression `e` reads, if deterministic,
    /// determine the attribute `a`.
    fn computed(&mut self, e: &Expr, a: usize) {
        if !deterministic(e) {
            return;
        }
        let mut read = Vec::new();
        expr_columns(e, &mut read);
        if let Some(from) = self.attributes_of(&read) {
            self.depends(from, Attrs::from([a]));
        }
    }

    /// [§4.26.11] It, then `other`, side by side: what is known of each
    /// holds of their product.
    fn beside(mut self, other: Known) -> Known {
        self.columns.extend(other.columns);
        self.dependencies.extend(other.dependencies);
        self.not_null.extend(other.not_null);
        self.nulled_with.extend(other.nulled_with);
        self.collations.extend(other.collations);
        self.pending.extend(self.keys.iter().cloned().chain(other.pending).chain(other.keys.iter().cloned()));
        self.keys.extend(other.keys);
        self.grouping = None;
        self
    }

    /// [§4.26.12] Of a condition over a product, the BUC-sets and BPK-sets
    /// of its table references made its own, of those that determine every
    /// column.
    fn adopted(mut self) -> Known {
        let pending = std::mem::take(&mut self.pending);
        self.keys.extend(pending);
        self.keyed(false)
    }

    /// [§4.26.7, §4.26.12] Its BUC-sets and BPK-sets that determine every
    /// column, and, of a joined table's if `not_null`, whose columns are
    /// all known not null.
    fn keyed(mut self, not_null: bool) -> Known {
        let visible: Attrs = self.columns.iter().copied().collect();
        let keys = std::mem::take(&mut self.keys);
        self.keys = keys
            .into_iter()
            .filter(|(key, _)| (!not_null || key.is_subset(&self.not_null)) && visible.is_subset(&self.closure(key.clone())))
            .collect();
        self
    }

    /// It, then `width` columns in all, the new ones of new attributes.
    fn widened(mut self, w: &Walk<'_>, width: usize) -> Known {
        let more = width.saturating_sub(self.columns.len());
        self.columns.extend((0..more).map(|_| w.attribute()));
        self
    }

    /// [§4.26.16] It, the first operand of an INTERSECT, and what is known
    /// of `second`, its second, of the same columns: each row of it is a
    /// row of both, so their dependencies hold of it. A column `second`
    /// has twice is the same in each row of it.
    fn meeting(mut self, second: &Known) -> Known {
        let mut map: Vec<(usize, usize)> = Vec::new();
        let mut alike = Vec::new();
        for (&s, &f) in second.columns.iter().zip(&self.columns) {
            match map.iter().find(|(from, _)| *from == s) {
                Some(&(_, to)) => alike.push((to, f)),
                None => map.push((s, f)),
            }
        }
        for (to, f) in alike {
            self.same(to, f);
        }
        let moved = |a: usize| map.iter().find(|(from, _)| *from == a).map_or(a, |(_, to)| *to);
        let moved_set = |s: &Attrs| s.iter().map(|&a| moved(a)).collect::<Attrs>();
        for (a, b) in &second.dependencies {
            self.depends(moved_set(a), moved_set(b));
        }
        self.not_null.extend(second.not_null.iter().map(|&a| moved(a)));
        self.keys.extend(second.keys.iter().map(|(key, primary)| (moved_set(key), *primary)));
        self.grouping = None;
        self
    }

    /// [§4.26.15] The attribute an expression `e` is, if it is a COALESCE
    /// of a column x and a column y [ISO/IEC 9075:1992 §6.9 SR2], as a
    /// NATURAL or USING join's common column is [§4.26.7]: x's, wherever x
    /// is known not null or y is null where x is.
    fn coalesced(&self, e: &Expr) -> Option<usize> {
        let Expr::Case(arms, other) = e else { return None };
        let [(Cond::IsNull(Expr::Column(tested), true), Expr::Column(x))] = arms.as_slice() else { return None };
        let Expr::Column(y) = &**other else { return None };
        let (ax, ay) = (self.attribute_of(*x)?, self.attribute_of(*y)?);
        let alike = tested == x && (self.not_null.contains(&ax) || self.nulled_with.contains(&(ax, ay)));
        alike.then_some(ax)
    }
}

/// What is known of the rows of `plan`: its node's, the keys of a
/// product's table references carried only to the condition over it.
fn known(w: &Walk<'_>, plan: &LogicalPlan) -> Known {
    let mut known = of_node(w, plan);
    if !matches!(plan, LogicalPlan::Product { .. }) {
        known.pending.clear();
    }
    known
}

/// What is known of the rows of `plan`, of its node's rule. Matched by
/// reference, each arm handing the plan's parts on, so a level of the plan
/// adds little to the stack, which the parser's nesting limit bounds
/// (RULES.md R-CF-6).
fn of_node(w: &Walk<'_>, plan: &LogicalPlan) -> Known {
    match plan {
        LogicalPlan::Scan { table, names, .. } => tables::base_table(w, table, names.len()),
        LogicalPlan::Filter { cond, input } => known(w, input).filtered(cond).adopted(),
        LogicalPlan::Product { left, right } => known(w, left).beside(known(w, right)).keyed(false),
        LogicalPlan::Aggregate { groups, aggs, seeds, input, .. } => grouped(w, known(w, input), groups, (aggs.len(), seeds.is_empty())),
        LogicalPlan::Project { exprs, input } => projected(w, known(w, input), exprs),
        // [§4.26.16] Rows of its input, once each or in order; or some of
        // them, as OFFSET and FETCH FIRST keep, of which all that holds of
        // them holds [NOTE 71].
        LogicalPlan::Distinct { input } | LogicalPlan::Sort { input, .. } | LogicalPlan::Limit { input, .. } => known(w, input),
        LogicalPlan::OuterJoin { kind, cond, left, right } => conditions::outer_joined(*kind, cond, (known(w, left), known(w, right))),
        LogicalPlan::SetOp { op, left, right, .. } => set_operation(w, *op, (left, right), plan.width()),
        LogicalPlan::Values { rows, types } => values(w, rows.len(), types.len()),
        LogicalPlan::Unnest(u) => unnested(w, u),
        // Rows of its input, each with the values of its window functions.
        LogicalPlan::Window(window) => known(w, &window.input).widened(w, plan.width()),
        // [§4.26] Of row pattern recognition, which §4.26 gives no rule,
        // nothing.
        LogicalPlan::Recognize(_) => w.fresh(plan.width()),
        // [§4.26] Of a <PTF derived table>, whose rows its component
        // procedures make, which §4.26 gives no rule, nothing.
        LogicalPlan::Polymorphic(p) => w.fresh(p.names.len()),
        // [§4.26.8, §4.26.16] Of a system table, of a recursive query's
        // (IA204), and of a transition table, which §4.26 gives no rule,
        // nothing.
        LogicalPlan::System { names, .. }
        | LogicalPlan::Recursive { names, .. }
        | LogicalPlan::WorkTable { names, .. }
        | LogicalPlan::Transition { names, .. } => w.fresh(names.len()),
    }
}

/// [§4.26.13] The groups of the rows `input` by its columns `groups`, then
/// a column for each of `aggs` set functions. Each grouping column keeps
/// its attribute, and they determine the rest [note 70]. What is known of
/// the rows holds of the groups, as each group's grouping columns are those
/// of any of its rows; but not where groups of no rows are made before any
/// row (`rowed` false), of grouping sets, whose rows are a union's. The
/// grouping columns are a BUC-set if each is known not null, and a BPK-set
/// if they are a subset of one of the rows'.
fn grouped(w: &Walk<'_>, input: Known, groups: &[usize], (aggs, rowed): (usize, bool)) -> Known {
    let mut columns: Vec<usize> = groups.iter().map(|&g| input.attribute_of(g).unwrap_or_else(|| w.attribute())).collect();
    let g: Attrs = columns.iter().copied().collect();
    let made: Vec<usize> = (0..aggs).map(|_| w.attribute()).collect();
    columns.extend(&made);
    let primary = input.keys.iter().any(|(key, primary)| *primary && g.is_subset(key));
    let not_null = g.is_subset(&input.not_null);
    let mut known = if rowed { Known { columns, ..input } } else { Known { columns, ..Known::default() } };
    known.keys.clear();
    if rowed && (primary || not_null) {
        known.keys.push((g.clone(), primary));
    }
    known.grouping = rowed.then(|| g.clone());
    known.depends(g, made.into_iter().collect());
    known
}

/// [§4.26.15] The select list `exprs` of the rows `known`: a column
/// reference's column keeps its attribute; another expression's column is
/// new, determined by the columns it references if it is deterministic,
/// and a COALESCE alike a column is alike it (`Known::coalesced`).
fn projected(w: &Walk<'_>, mut known: Known, exprs: &[Expr]) -> Known {
    let mut columns = Vec::with_capacity(exprs.len());
    for e in exprs {
        if let Expr::Column(c) = e
            && let Some(a) = known.attribute_of(*c)
        {
            columns.push(a);
            continue;
        }
        let a = w.attribute();
        known.computed(e, a);
        if let Some(x) = known.coalesced(e) {
            known.same(x, a);
        }
        columns.push(a);
    }
    // [§4.26.15] A BUC-set or BPK-set of columns the select list
    // references is one of its result.
    let visible: Attrs = columns.iter().copied().collect();
    known.keys.retain(|(key, _)| key.is_subset(&visible));
    known.columns = columns;
    known
}

/// [§4.26.16] A set operation `op` of `width` columns of its operands:
/// UNION's rows are made anew, of which nothing is known (IA226: nothing
/// added); EXCEPT's are rows of its first operand, and INTERSECT's of both.
fn set_operation(w: &Walk<'_>, op: SetOp, (left, right): (&LogicalPlan, &LogicalPlan), width: usize) -> Known {
    match op {
        SetOp::Union => w.fresh(width),
        SetOp::Except => Known { grouping: None, ..known(w, left) },
        SetOp::Intersect => known(w, left).meeting(&known(w, right)),
    }
}

/// [§4.26.6] A table value constructor of `rows` rows of `width` columns:
/// nothing (IA203), but of one row or none, every column determined by none
/// (IE015).
fn values(w: &Walk<'_>, rows: usize, width: usize) -> Known {
    let mut known = w.fresh(width);
    if rows <= 1 {
        known.depends(Attrs::new(), known.columns.iter().copied().collect());
    }
    known
}

/// [§4.26.8] A collection derived table over the rows of its input: each
/// is a row of it with elements of its collections, so what is known of
/// the input holds. WITH ORDINALITY, the input's columns and the
/// ordinality column determine its own columns, its collections and
/// condition being deterministic: of an input of no columns, the
/// ordinality column alone, as the rule gives; of another, they with it
/// (IE015), as the ordinal starts again for each row of the input.
fn unnested(w: &Walk<'_>, u: &Unnest<LogicalPlan>) -> Known {
    let mut known = known(w, &u.input);
    let own: Vec<usize> = (0..u.width()).map(|_| w.attribute()).collect();
    let fixed = u.collections.iter().all(deterministic) && u.outer.as_ref().is_none_or(conditions::deterministic_cond);
    if let Some(&ordinal) = own.last().filter(|_| u.ordinality && fixed) {
        let mut from: Attrs = known.columns.iter().copied().collect();
        from.insert(ordinal);
        known.depends(from, own.iter().copied().collect());
        if u.outer.is_none() {
            known.not_null.insert(ordinal);
        }
    }
    // [§4.26.8] The ordinality column is a BPK-set of a collection derived
    // table that reads no column before it; of one that does, the rows'
    // keys are not, as each row of them is given many.
    let alone = known.columns.is_empty() && u.outer.is_none();
    known.keys = own.last().filter(|_| u.ordinality && fixed && alone).map(|&o| (Attrs::from([o]), true)).into_iter().collect();
    known.grouping = None;
    known.columns.extend(own);
    known
}
