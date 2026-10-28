//! [X3.135-1986 §5.10-§5.17; ISO/IEC 9075:1992 §8] Predicates, bound. A
//! comparison of rows, BETWEEN and IN of a value list are bound as the
//! scalar comparisons the standard defines them by [§8.2 GR7, §8.3, §8.4],
//! so the planner sees, and an index can answer, plain comparisons; those
//! with a `<table subquery>` as a comparison with its rows
//! (`Cond::Quantified`). Split from binder.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::expression::dynamic_of;
use super::subquery::Use;
use super::{Binder, Scope, rule};
use crate::ast::{Predicate, RowValue, Subquery, ValueExpression};
use crate::error::{At, Result};
use crate::logical::{Cond, Expr, Quantifier, Truth, and_all};
use crate::params::ParameterType;
use crate::value::{CompOp, DataType};

/// `cond`, or NOT `cond` if `not`.
fn negated(cond: Cond, not: bool) -> Cond {
    if not { Cond::Not(Box::new(cond)) } else { cond }
}

/// The disjunction of `conds`; false if there are none.
fn or_all(conds: Vec<Cond>) -> Cond {
    crate::logical::or_all(conds).unwrap_or(Cond::Const(Truth::False))
}

/// A bound row: its values and their types.
type BoundRow = Vec<(Expr, DataType)>;

/// Whether `values` is a `<row subquery>` alone.
fn is_row_subquery(values: &RowValue) -> bool {
    matches!(values.as_slice(), [ValueExpression::Subquery(_)])
}

/// The types of a bound row.
fn types(row: &BoundRow) -> Vec<DataType> {
    row.iter().map(|(_, t)| *t).collect()
}

/// The expressions of a bound row.
fn exprs(row: &[(Expr, DataType)]) -> Vec<Expr> {
    row.iter().map(|(e, _)| e.clone()).collect()
}

/// [ISO/IEC 9075:1992 §8.2 SR1, SR3] Rows of one degree, their
/// corresponding values comparable; `clause` is the rule to cite.
fn comparable(x: &[(Expr, DataType)], y: &[DataType], clause: &str) -> Result<()> {
    if x.len() != y.len() {
        return Err(rule(format!("[ISO/IEC 9075:1992 {clause}] rows of degrees {} and {} are compared", x.len(), y.len())));
    }
    for ((_, a), b) in x.iter().zip(y) {
        if !a.comparable(b) {
            return Err(rule(format!("[ISO/IEC 9075:1992 {clause}] {a} and {b} are not comparable")));
        }
    }
    Ok(())
}

/// [ISO/IEC 9075:1992 §8.2 GR7] "Rx op Ry", rows of one degree, as scalar
/// comparisons: = all pairs equal; <> one unequal; < and > the pairs equal
/// up to one less or greater; <= and >= the negation of > and <, which
/// [GR7i-l] makes them, in three-valued logic. One pair is "x op y".
fn compare_rows(x: &[Expr], op: CompOp, y: &[Expr]) -> Cond {
    if let ([a], [b]) = (x, y) {
        return Cond::Compare(a.clone(), op, b.clone());
    }
    let pairs = |pair_op| x.iter().zip(y).map(move |(a, b)| Cond::Compare(a.clone(), pair_op, b.clone()));
    match op {
        CompOp::Eq => and_all(pairs(CompOp::Eq).collect()).unwrap_or(Cond::Const(Truth::True)),
        CompOp::Ne => or_all(pairs(CompOp::Ne).collect()),
        CompOp::Lt | CompOp::Gt => {
            let equal: Vec<Cond> = pairs(CompOp::Eq).collect();
            let each = pairs(op).enumerate().map(|(n, decided)| {
                let before = equal.get(..n).unwrap_or_default().iter().cloned();
                and_all(before.chain(std::iter::once(decided)).collect()).unwrap_or(Cond::Const(Truth::True))
            });
            or_all(each.collect())
        }
        CompOp::Le => Cond::Not(Box::new(compare_rows(x, CompOp::Gt, y))),
        CompOp::Ge => Cond::Not(Box::new(compare_rows(x, CompOp::Lt, y))),
    }
}

impl Binder<'_> {
    /// [X3.135-1986 §5.10; ISO/IEC 9075:1992 §8.1] a `<predicate>`.
    pub(super) fn predicate(&self, p: &Predicate, scope: Scope<'_>) -> Result<Cond> {
        Ok(match p {
            Predicate::Comparison(left, op, right) => {
                let (x, y) = self.rows_paired(left, right, scope)?;
                comparable(&x, &y.iter().map(|(_, t)| *t).collect::<Vec<_>>(), "§8.2 SR1, SR3")?;
                compare_rows(&exprs(&x), *op, &exprs(&y))
            }
            Predicate::Between { value, low, high, not } => self.between([value, low, high], *not, scope)?,
            Predicate::Overlaps(left, right) => self.overlaps(left, right, scope)?,
            Predicate::InList { value, list, not } => self.in_list(value, list, *not, scope)?,
            // [§8.4 SR3, SR4] "R IN S" is "R = ANY S".
            Predicate::InSubquery { value, subquery, not } => {
                negated(self.quantified(value, (CompOp::Eq, Quantifier::Some), subquery, scope)?, *not)
            }
            Predicate::Quantified { value, op, all, subquery } => {
                let quantifier = if *all { Quantifier::All } else { Quantifier::Some };
                self.quantified(value, (*op, quantifier), subquery, scope)?
            }
            Predicate::Like { value, pattern, escape, not } => self.like([value, pattern], escape.as_deref(), *not, scope)?,
            Predicate::Null { value, not } => self.null(value, *not, scope)?,
            Predicate::Exists(s) => Cond::Exists(self.subquery(s, scope, Use::Exists)?.0),
            Predicate::Unique(s) => Cond::Unique(self.subquery(s, scope, Use::Rows)?.0),
            Predicate::Match { value, unique, match_type, subquery } => {
                // [§17.6 GR5t] a `?` takes its subquery column's type.
                let (s, types) = self.subquery(subquery, scope, Use::Rows)?;
                let x = self.row_as(value, &types, scope)?;
                comparable(&x, &types, "§8.10 SR1, SR2")?;
                Cond::Match { values: exprs(&x), unique: *unique, match_type: *match_type, subquery: s }
            }
        })
    }

    /// [ISO/IEC 9075:1992 §7.1] a `<row value constructor>`: its values and
    /// their types. A `<row subquery>`, which is written as a `<scalar
    /// subquery>`, gives one value for each of its columns [§7.11].
    pub(super) fn row(&self, values: &RowValue, scope: Scope<'_>) -> Result<Vec<(Expr, DataType)>> {
        if let [ValueExpression::Subquery(s)] = values.as_slice() {
            let (s, types) = self.subquery(s, scope, Use::Rows)?;
            return Ok(types.into_iter().enumerate().map(|(column, t)| (Expr::Subquery(Arc::clone(&s), column), t)).collect());
        }
        values.iter().map(|v| self.value_expression(v, scope)).collect()
    }

    /// [ISO/IEC 9075:1992 §17.6 GR5m] Two rows compared, bound: a `<dynamic
    /// parameter specification>` in one taking the type of the value in the
    /// same place of the other, a row subquery's column's among them; one
    /// in both places, none \[GR4i\].
    pub(super) fn rows_paired(&self, left: &RowValue, right: &RowValue, scope: Scope<'_>) -> Result<(BoundRow, BoundRow)> {
        if is_row_subquery(right) {
            let y = self.row(right, scope)?;
            return Ok((self.row_as(left, &types(&y), scope)?, y));
        }
        if is_row_subquery(left) || left.len() != right.len() {
            let x = self.row(left, scope)?;
            return Ok((x.clone(), self.row_as(right, &types(&x), scope)?));
        }
        let (mut x, mut y) = (Vec::new(), Vec::new());
        for (a, b) in left.iter().zip(right) {
            let [p, q] = self.pair((a, b), scope)?;
            x.push(p);
            y.push(q);
        }
        Ok((x, y))
    }

    /// `values`, a row, bound: a `<dynamic parameter specification>` among
    /// them taking the type in the same place of `taken` [§17.6 GR5m, q, t].
    pub(super) fn row_as(&self, values: &RowValue, taken: &[DataType], scope: Scope<'_>) -> Result<BoundRow> {
        if is_row_subquery(values) {
            return self.row(values, scope);
        }
        let typed = |(i, v): (usize, &ValueExpression)| match taken.get(i) {
            Some(t) => self.typed(v, ParameterType::Of(*t), scope),
            None => self.value_expression(v, scope),
        };
        values.iter().enumerate().map(typed).collect()
    }

    /// [X3.135-1986 §5.16; ISO/IEC 9075:1992 §8.7] "R op ALL S" or "R op
    /// SOME S": R and the rows of S of one degree, comparable [SR1-SR3].
    fn quantified(&self, value: &RowValue, (op, quantifier): (CompOp, Quantifier), s: &Subquery, scope: Scope<'_>) -> Result<Cond> {
        // [§17.6 GR5q, t] a `?` takes its subquery column's type.
        let (bound, types) = self.subquery(s, scope, Use::Rows)?;
        let x = self.row_as(value, &types, scope)?;
        comparable(&x, &types, "§8.7 SR1, SR2").or_at(s.at)?;
        Ok(Cond::Quantified(exprs(&x), op, quantifier, bound))
    }

    /// [ISO/IEC 9075:1992 §8.3] "X BETWEEN Y AND Z" is "X >= Y AND X <= Z";
    /// "X NOT BETWEEN Y AND Z" is "NOT (X BETWEEN Y AND Z)", of rows of one
    /// degree, comparable.
    pub(super) fn between(&self, rows: [&RowValue; 3], not: bool, scope: Scope<'_>) -> Result<Cond> {
        let [value, low, high] = rows;
        let (x, y, z) = self.between_rows([value, low, high], scope)?;
        for other in [&y, &z] {
            comparable(&x, &other.iter().map(|(_, t)| *t).collect::<Vec<_>>(), "§8.3")?;
        }
        let at_least = compare_rows(&exprs(&x), CompOp::Ge, &exprs(&y));
        let at_most = compare_rows(&exprs(&x), CompOp::Le, &exprs(&z));
        Ok(negated(Cond::And(Box::new(at_least), Box::new(at_most)), not))
    }

    /// [ISO/IEC 9075:1992 §17.6 GR5m] The rows of "X BETWEEN Y AND Z",
    /// bound: a `<dynamic parameter specification>` in Y or Z taking the
    /// type of X's value in its place, one in X the type of Y's; one in X
    /// and in Y or Z, none \[GR4i\].
    fn between_rows(&self, [value, low, high]: [&RowValue; 3], scope: Scope<'_>) -> Result<(BoundRow, BoundRow, BoundRow)> {
        let same = !is_row_subquery(value) && value.len() == low.len() && value.len() == high.len();
        if !same {
            let bound = self.row(value, scope)?;
            return Ok((bound.clone(), self.row_as(low, &types(&bound), scope)?, self.row_as(high, &types(&bound), scope)?));
        }
        let (mut values, mut lows, mut highs) = (Vec::new(), Vec::new(), Vec::new());
        for ((a, b), c) in value.iter().zip(low).zip(high) {
            let by_low = dynamic_of(a).is_some() && dynamic_of(b).is_none() && dynamic_of(c).is_none();
            let (tested, from) = if by_low {
                let from = self.value_expression(b, scope)?;
                (self.typed(a, ParameterType::Of(from.1), scope)?, from)
            } else {
                let tested = self.value_expression(a, scope)?;
                let from = self.typed(b, ParameterType::Of(tested.1), scope)?;
                (tested, from)
            };
            highs.push(self.typed(c, ParameterType::Of(tested.1), scope)?);
            values.push(tested);
            lows.push(from);
        }
        Ok((values, lows, highs))
    }

    /// [ISO/IEC 9075:1992 §8.4] "x IN (v1, v2, ...)" is "x = ANY (VALUES v1,
    /// v2, ...)" [SR1, SR4]: true if x equals one, false if it equals none,
    /// else unknown [§8.7 GR], which is "x = v1 OR x = v2 OR ..." by the
    /// truth table of OR [X3.135-1986 §5.18 GR2]. "x NOT IN S" is "NOT (x IN
    /// S)" [SR3].
    pub(super) fn in_list(&self, value: &RowValue, list: &[ValueExpression], not: bool, scope: Scope<'_>) -> Result<Cond> {
        // [§17.6 GR5r] a `?` takes the type D of the row, or if that is one,
        // of the first value of the list; that one too, none [GR4k].
        let lone = value.first().filter(|v| value.len() == 1 && dynamic_of(v).is_some());
        let x = match (lone, list.first().filter(|v| dynamic_of(v).is_none())) {
            (Some(v), Some(first)) => {
                let (_, d) = self.value_expression(first, scope)?;
                vec![self.typed(v, ParameterType::Of(d), scope)?]
            }
            (Some(_), None) | (None, _) => self.row(value, scope)?,
        };
        let d = match x.as_slice() {
            [(_, t)] => Some(*t),
            _ => None,
        };
        let mut any = Vec::new();
        for v in list {
            let (e, t) = match d {
                Some(d) => self.typed(v, ParameterType::Of(d), scope)?,
                None => self.value_expression(v, scope)?,
            };
            comparable(&x, &[t], "§8.4 SR1, SR4; §8.7 SR1, SR2")?;
            any.push(compare_rows(&exprs(&x), CompOp::Eq, &[e]));
        }
        Ok(negated(or_all(any), not))
    }

    /// [ISO/IEC 9075:1992 §8.5] `<like predicate>`: the `<match value>`,
    /// `<pattern>` and `<escape character>`, character strings [SR1].
    pub(super) fn like(
        &self,
        [value, pattern]: [&ValueExpression; 2],
        escape: Option<&ValueExpression>,
        not: bool,
        scope: Scope<'_>,
    ) -> Result<Cond> {
        let mut bound = Vec::new();
        for (what, v) in [("<match value>", Some(value)), ("<pattern>", Some(pattern)), ("<escape character>", escape)] {
            let Some(v) = v else { continue };
            // [§17.6 GR5s] a `?` is CHARACTER VARYING.
            let (e, t) = self.typed(v, ParameterType::Varying, scope)?;
            if !t.is_character() {
                return Err(rule(format!("[ISO/IEC 9075:1992 §8.5 SR1] the {what} of LIKE is a character string, not {t}")));
            }
            bound.push(e);
        }
        let mut bound = bound.into_iter();
        let (Some(matched), Some(against)) = (bound.next(), bound.next()) else {
            return Err(rule("[ISO/IEC 9075:1992 §8.5 F] LIKE has a match value and a pattern"));
        };
        Ok(negated(Cond::Like(matched, against, bound.next()), not))
    }

    /// [ISO/IEC 9075:1992 §8.6] "R IS NULL": all of R null [GR2]; "R IS
    /// NOT NULL": none of it [GR3], which for a row of more than one value
    /// is not "NOT (R IS NULL)".
    fn null(&self, value: &RowValue, not: bool, scope: Scope<'_>) -> Result<Cond> {
        let each: Vec<Cond> = self.row(value, scope)?.into_iter().map(|(e, _)| Cond::IsNull(e, not)).collect();
        and_all(each).ok_or_else(|| rule("[ISO/IEC 9075:1992 §8.6] IS NULL tests a value"))
    }
}
