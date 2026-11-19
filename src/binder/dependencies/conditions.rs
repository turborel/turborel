//! [ISO/IEC 9075-2:2023 §4.26.2, §4.26.7, §4.26.12, §4.26.14] What a
//! condition makes known of the rows it is true of, each equality
//! AND-component of it and each column it compares; what an outer join
//! keeps of its operands' dependencies and adds of its condition; and
//! which expressions are deterministic, as those rules require.

use super::{Attrs, Known};
use crate::logical::{Cond, Expr, OuterJoin, cond_columns, expr_columns};
use crate::value::CompOp;

/// Whether `e` gives one value of one row: no subquery, no NEXT VALUE FOR
/// and no function or routine that is not deterministic [§9.16]. A parameter and an
/// outer reference are one value in each evaluation, as is a datetime
/// value function in a statement.
pub(in crate::binder) fn deterministic(e: &Expr) -> bool {
    match e {
        Expr::Column(_) | Expr::Const(_) | Expr::Outer(..) | Expr::Dynamic(_) => true,
        Expr::Neg(a) | Expr::Cast(a, _) | Expr::Domain(a, _) | Expr::SortKey(a, _) | Expr::Warned(a, _) => deterministic(a),
        Expr::Arith(a, _, b) => deterministic(a) && deterministic(b),
        Expr::Call(f, args) => f.0.deterministic() && args.iter().all(deterministic),
        Expr::Case(arms, other) => arms.iter().all(|(c, v)| deterministic_cond(c) && deterministic(v)) && deterministic(other),
        Expr::Truth(c) => deterministic_cond(c),
        Expr::Subquery(..) | Expr::NextValue(_) => false,
        // [ISO/IEC 9075-2:2023 §9.16 GR1] a deterministic routine's value
        // is the one of the same arguments.
        Expr::Invoked(i) => i.deterministic && i.args.iter().all(deterministic),
    }
}

/// Whether the condition `c` is deterministic: of deterministic
/// expressions, and no subquery.
pub(super) fn deterministic_cond(c: &Cond) -> bool {
    match c {
        Cond::Const(_) => true,
        Cond::Or(a, b) | Cond::And(a, b) => deterministic_cond(a) && deterministic_cond(b),
        Cond::Not(a) | Cond::Is(a, ..) => deterministic_cond(a),
        Cond::Compare(a, _, b) | Cond::Distinct(a, b) => deterministic(a) && deterministic(b),
        Cond::IsNull(a, _) | Cond::Value(a) | Cond::In(a, _) => deterministic(a),
        Cond::Like(a, b, escape) => deterministic(a) && deterministic(b) && escape.as_ref().is_none_or(deterministic),
        Cond::Exists(_) | Cond::Quantified(..) | Cond::Unique(..) | Cond::Match { .. } => false,
    }
}

/// [§4.26.2] The AND-components of `c`, added to `out`.
fn and_components<'c>(c: &'c Cond, out: &mut Vec<&'c Cond>) {
    if let Cond::And(a, b) = c {
        and_components(a, out);
        and_components(b, out);
    } else {
        out.push(c);
    }
}

/// Whether `e` is deterministic and references no column: one value in
/// all the rows, as a `<literal>` is [§4.26.12 note 68].
fn constant(e: &Expr) -> bool {
    let mut read = Vec::new();
    expr_columns(e, &mut read);
    read.is_empty() && deterministic(e)
}

/// The column `e` is, a null where the column is: the column, or a sort
/// key of it.
fn bare_column(e: &Expr) -> Option<usize> {
    match e {
        Expr::Column(c) => Some(*c),
        Expr::SortKey(a, _) => bare_column(a),
        Expr::Const(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Case(..)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::Warned(..)
        | Expr::Subquery(..)
        | Expr::Truth(_)
        | Expr::NextValue(_)
        | Expr::Invoked(_) => None,
    }
}

impl Known {
    /// The column `e` is a reference to, compared by its own values: the
    /// column itself, or its sort key by the collation of its base table's
    /// column, which compares its values as they are told apart.
    fn compared_column(&self, e: &Expr) -> Option<usize> {
        match e {
            Expr::Column(c) => Some(*c),
            Expr::SortKey(key, by) => {
                let Expr::Column(c) = &**key else { return None };
                let attribute = self.attribute_of(*c)?;
                self.collations.iter().any(|(at, own)| *at == attribute && own == by).then_some(*c)
            }
            Expr::Const(_)
            | Expr::Neg(_)
            | Expr::Arith(..)
            | Expr::Call(..)
            | Expr::Outer(..)
            | Expr::Dynamic(_)
            | Expr::Case(..)
            | Expr::Cast(..)
            | Expr::Domain(..)
            | Expr::Warned(..)
            | Expr::Subquery(..)
            | Expr::Truth(_)
            | Expr::NextValue(_)
            | Expr::Invoked(_) => None,
        }
    }

    /// [§4.26.12, §4.26.14] The rows of it `cond` is true of: what is
    /// known of it holds of them, and of each equality AND-component of
    /// `cond`, as `equal` says; each column an AND-component compares, or
    /// says IS NOT NULL of, is not null in them.
    pub(super) fn filtered(mut self, cond: &Cond) -> Known {
        let mut components = Vec::new();
        and_components(cond, &mut components);
        for c in components {
            match c {
                Cond::Compare(l, op, r) => {
                    self.compared(l);
                    self.compared(r);
                    if *op == CompOp::Eq {
                        self.equal(l, r);
                    }
                }
                Cond::IsNull(e, true) => self.compared(e),
                Cond::IsNull(_, false)
                | Cond::Const(_)
                | Cond::Or(..)
                | Cond::And(..)
                | Cond::Not(_)
                | Cond::Distinct(..)
                | Cond::Like(..)
                | Cond::Exists(_)
                | Cond::Quantified(..)
                | Cond::Is(..)
                | Cond::Unique(..)
                | Cond::Match { .. }
                | Cond::Value(_)
                | Cond::In(..) => {}
            }
        }
        self
    }

    /// That the column `e` is, if it is one, is not null: a comparison
    /// true of it is of a value.
    fn compared(&mut self, e: &Expr) {
        if let Some(a) = bare_column(e).and_then(|c| self.attribute_of(c)) {
            self.not_null.insert(a);
        }
    }

    /// [§4.26.12] An equality AND-component of `l` and `r`: of two column
    /// references, each determines the other; of a column reference and a
    /// `<literal>`, or another deterministic expression of no column [note
    /// 68], none determines the column; of a column reference and a
    /// deterministic expression of columns, they determine it [note 69].
    fn equal(&mut self, left: &Expr, right: &Expr) {
        let attribute = |e: &Expr| self.compared_column(e).and_then(|column| self.attribute_of(column));
        match (attribute(left), attribute(right)) {
            (Some(first), Some(second)) => self.same(first, second),
            (Some(first), None) => self.computed(right, first),
            (None, Some(second)) => self.computed(left, second),
            (None, None) => {}
        }
    }

    /// [§4.26.7] Of an outer join, the operand whose rows may be extended
    /// with nulls: only its dependencies A ↦ B of its own columns where a
    /// column of A is known not null hold, as an extended row has a null
    /// there and a row of it has not; none of its columns is known not
    /// null.
    pub(super) fn null_extended(mut self) -> Known {
        let visible: Attrs = self.columns.iter().copied().collect();
        let not_null = std::mem::take(&mut self.not_null);
        self.dependencies.retain(|(a, b)| a.is_subset(&visible) && b.is_subset(&visible) && a.iter().any(|x| not_null.contains(x)));
        self
    }

    /// [§4.26.7] Of a LEFT join if `left`, else a RIGHT one, on a
    /// deterministic condition `cond`, of `width` columns of its first
    /// operand: the columns of the preserved operand `cond` references
    /// (DJA, or DJB) determine each column of the other that an equality
    /// AND-component of it compares with a constant or with a column of
    /// the preserved operand; and that column is null wherever that one is,
    /// as no row matches it then.
    fn preserving(&mut self, cond: &Cond, width: usize, left: bool) {
        let preserved = |c: usize| (c < width) == left;
        let mut read = Vec::new();
        cond_columns(cond, &mut read);
        read.retain(|&c| preserved(c));
        let Some(dja) = self.attributes_of(&read) else { return };
        let mut components = Vec::new();
        and_components(cond, &mut components);
        for component in components {
            let Cond::Compare(l, CompOp::Eq, r) = component else { continue };
            for (p, q) in [(l, r), (r, l)] {
                let Some(y) = bare_column(q).filter(|&c| !preserved(c)).and_then(|c| self.attribute_of(c)) else { continue };
                if let Some(x) = bare_column(p).filter(|&c| preserved(c)).and_then(|c| self.attribute_of(c)) {
                    self.nulled_with.push((x, y));
                }
                let compared = self.compared_column(q).is_some_and(|c| !preserved(c));
                if compared && (constant(p) || self.compared_column(p).is_some_and(preserved)) {
                    self.depends(dja.clone(), Attrs::from([y]));
                }
            }
        }
    }
}

/// [§4.26.7] A LEFT, RIGHT or FULL join `kind` of the rows `left` and
/// `right` on `cond`: of each operand whose rows may be extended with
/// nulls, only what `Known::null_extended` keeps; of the preserved one,
/// what is known; of a LEFT or RIGHT join on a deterministic condition,
/// what `Known::preserving` adds; of each operand's BUC-sets and BPK-sets,
/// those `Known::keyed` keeps. A partitioned join's plan is read as the
/// nodes it is made of, its join partitioning columns' rules not applied.
pub(super) fn outer_joined(kind: OuterJoin, cond: &Cond, (left, right): (Known, Known)) -> Known {
    let width = left.columns.len();
    let mut known = match kind {
        OuterJoin::Left => left.beside(right.null_extended()),
        OuterJoin::Right => left.null_extended().beside(right),
        OuterJoin::Full => left.null_extended().beside(right.null_extended()),
    };
    if deterministic_cond(cond) {
        match kind {
            OuterJoin::Left => known.preserving(cond, width, true),
            OuterJoin::Right => known.preserving(cond, width, false),
            OuterJoin::Full => {}
        }
    }
    known.keyed(true)
}
