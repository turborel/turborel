//! [ISO/IEC 9075-2:2023 §9.15; ISO/IEC 9075:1992 §4.2.3] Which collation a
//! comparison, grouping or ordering of character strings is by (RULES.md
//! D60), found from the statement's text, beside its binding: a column is
//! implicitly of its own, a `<collate clause>` is explicit, and an operation
//! of two takes what §9.5 says.
//!
//! \[EXTENSION\] A literal, parameter, CAST or USER yields to a column's
//! collation, as SQL-92's Coercible did. §9.15 SR4e read alone would refuse
//! `C = 'x'` of any collated column C (CONFORMANCE.md).
//!
//! A comparison by the default stays bare. One by another compares sort
//! keys (`Expr::sort_key`).

use std::sync::Arc;

use super::predicates::BoundRow;
use super::{Binder, Bound, OutColumn, Scope, rule};
use crate::ast::{
    CaseExpression, ColumnSpecification, RowValue, SetArg, SortSpecification, Subquery as AstSubquery, TableName, ValueExpression,
};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::exec::{Escape, like_escape};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::logical::{Cond, Expr, LogicalPlan, SortKey, Subquery};
use crate::system::INFORMATION_SCHEMA;
use crate::value::collation::{CollationRef, SQL_TEXT};
use crate::value::{CompOp, DataType, Decimal, Spelled, Value, like_collated};

/// The collations of each subquery's columns, by the subquery's text.
pub(super) type SubqueryCollations = Vec<(*const AstSubquery, Vec<Option<QualifiedName>>)>;

/// The length of a collation's name, an SQL_IDENTIFIER's [§21.2.28].
const MAX_IDENTIFIER_LENGTH: u32 = 128;

/// How a value came by its collation [ISO/IEC 9075-2:2023 §4.3.2].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Derived {
    /// \[EXTENSION\] It takes what the other operand has.
    Yielding,
    /// A column's, none being the character set's.
    Implicit(Option<QualifiedName>),
    /// A `<collate clause>`'s.
    Explicit(QualifiedName),
    /// None: an operation of operands implicitly of two [§9.5].
    Conflicting,
}

/// [ISO/IEC 9075-2:2023 §9.5] What an operation of `a` and `b` has.
fn combined(a: Derived, b: Derived) -> Result<Derived> {
    Ok(match (a, b) {
        (Derived::Explicit(x), Derived::Explicit(y)) if x != y => {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §9.5] the collations {x} and {y} are both explicit")));
        }
        (e @ Derived::Explicit(_), _) | (_, e @ Derived::Explicit(_)) => e,
        (Derived::Yielding, other) | (other, Derived::Yielding) => other,
        (Derived::Implicit(x), Derived::Implicit(y)) if x == y => Derived::Implicit(x),
        (Derived::Implicit(_) | Derived::Conflicting, Derived::Implicit(_) | Derived::Conflicting) => Derived::Conflicting,
    })
}

/// [ISO/IEC 9075:1992 §8.5] A character folded as its collation folds it:
/// LIKE of folded strings is LIKE under the collation.
struct Folded(CollationRef);

impl ScalarFunction for Folded {
    fn name(&self) -> &str {
        self.0.collation.name()
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        Ok(args.first().copied().unwrap_or(DataType::Varying { length: 0 }))
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(Value::Char(text)) = args.first() else { return Ok(args.first().cloned().unwrap_or(Value::Null)) };
        Ok(Value::Char(text.chars().map(|c| self.0.collation.fold(c).unwrap_or(c)).collect()))
    }
}

/// [ISO/IEC 9075-2:2023 §8.5 GR3d] LIKE under a collation whose characters
/// do not compare one at a time: one if the match value, the pattern and
/// the escape character, its arguments, match, zero if not, null if one is
/// null.
struct Collated(CollationRef);

impl ScalarFunction for Collated {
    fn name(&self) -> &str {
        self.0.collation.name()
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Exact { precision: 1, scale: 0, spelled: Spelled::Smallint })
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let escape = match like_escape(args.get(2))? {
            Escape::None => None,
            Escape::Of(c) => Some(c),
            Escape::Null => return Ok(Value::Null),
        };
        let (Some(Value::Char(value)), Some(Value::Char(pattern))) = (args.first(), args.get(1)) else { return Ok(Value::Null) };
        let matched = like_collated(value, pattern, escape, &self.0)?;
        Ok(Value::Exact(Decimal::new(i128::from(matched), 0)))
    }
}

/// [ISO/IEC 9075:1992 §8.5] `e`, an operand of LIKE under `collation`,
/// whose characters compare one at a time: each folded.
fn liked(e: Expr, collation: &CollationRef) -> Expr {
    Expr::Call(ScalarRef(Arc::new(Folded(collation.clone()))), vec![e])
}

/// [ISO/IEC 9075-2:2023 §8.5] LIKE of `operands`, the match value, the
/// pattern and the escape character if one was given, under `collation`:
/// by each character folded if they compare one at a time, otherwise by
/// substrings equal under it (IA060).
pub(super) fn like_under(operands: Vec<Expr>, collation: &CollationRef) -> Cond {
    if collation.collation.fold('a').is_some() {
        let mut folded = operands.into_iter().map(|e| liked(e, collation));
        let (matched, against) = (folded.next().unwrap_or(Expr::Const(Value::Null)), folded.next().unwrap_or(Expr::Const(Value::Null)));
        return Cond::Like(matched, against, folded.next());
    }
    let call = Expr::Call(ScalarRef(Arc::new(Collated(collation.clone()))), operands);
    Cond::Compare(call, CompOp::Eq, Expr::Const(Value::Exact(Decimal::new(1, 0))))
}

/// The collation `name`, unless the default: a comparison by it stays bare.
fn effective(db: &State, name: &QualifiedName) -> Option<CollationRef> {
    db.collation_named(name).filter(|c| !c.is_default())
}

/// [ISO/IEC 9075-2:2023 §9.15 SR4] The collation of an operation on
/// `operands`, `session` the session's collation if it applies, none if
/// the default.
pub(super) fn determined(db: &State, session: Option<&CollationRef>, operands: &[Derived]) -> Result<Option<CollationRef>> {
    let mut explicit: Option<&QualifiedName> = None;
    for operand in operands {
        if let Derived::Explicit(name) = operand {
            // [SR4b]
            if explicit.is_some_and(|e| e != name) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §9.15 SR4b] the collations {} and {name} are both explicit",
                    explicit.map_or(String::new(), ToString::to_string)
                )));
            }
            explicit = Some(name);
        }
    }
    if let Some(name) = explicit {
        return Ok(effective(db, name));
    }
    // [SR4c]
    if let Some(session) = session {
        return Ok(Some(session.clone()).filter(|c| !c.is_default()));
    }
    // [SR4e]
    let mut implicit: Option<&Option<QualifiedName>> = None;
    for operand in operands {
        match operand {
            Derived::Implicit(name) if implicit.is_none_or(|i| i == name) => implicit = Some(name),
            Derived::Implicit(_) | Derived::Conflicting => {
                return Err(rule("[ISO/IEC 9075-2:2023 §9.15 SR4e] the operands are of different collations: write COLLATE"));
            }
            Derived::Yielding | Derived::Explicit(_) => {}
        }
    }
    Ok(implicit.and_then(Option::as_ref).and_then(|name| effective(db, name)))
}

/// [ISO/IEC 9075:1992 §7.5 SR6c, ISO/IEC 9075-2:2023 §9.15] The collation
/// corresponding join columns of collations `a` and `b` are compared by.
pub(super) fn of_join_columns(
    db: &State,
    session: Option<&CollationRef>,
    a: Option<&QualifiedName>,
    b: Option<&QualifiedName>,
) -> Result<Option<CollationRef>> {
    determined(db, session, &[Derived::Implicit(a.cloned()), Derived::Implicit(b.cloned())])
}

/// [§8.2 GR3d] The collation an operation that holds rows the same treats
/// column `c` of a query by.
pub(super) fn of_column(db: &State, session: Option<&CollationRef>, c: &OutColumn) -> Option<CollationRef> {
    if !c.data_type.is_character() {
        return None;
    }
    determined(db, session, &[Derived::Implicit(c.collation.clone())]).ok().flatten()
}

/// `collations` if any is not the default, else none: a plan node with
/// none is as it would be without collations.
pub(super) fn any_of(collations: Vec<Option<CollationRef>>) -> Vec<Option<CollationRef>> {
    if collations.iter().any(Option::is_some) { collations } else { Vec::new() }
}

/// `s`, its rows' columns under `collations` by their sort keys.
fn keyed_subquery(s: &Arc<Subquery>, collations: &[Option<CollationRef>]) -> Arc<Subquery> {
    let keyed = |(i, c): (usize, &Option<CollationRef>)| match c {
        Some(c) => Expr::sort_key(Expr::Column(i), c),
        None => Expr::Column(i),
    };
    let exprs = collations.iter().enumerate().map(keyed).collect();
    Arc::new(Subquery { plan: LogicalPlan::Project { exprs, input: Box::new(s.plan.clone()) }, outer: s.outer.clone() })
}

impl Binder<'_> {
    /// How `e`, a value of this query, came by its collation.
    pub(super) fn derived(&self, e: &ValueExpression) -> Result<Derived> {
        Ok(match e {
            ValueExpression::Column(cs) => self.column_derived(cs)?,
            ValueExpression::Nested(x) | ValueExpression::Monadic(_, x) => self.derived(x)?,
            ValueExpression::Collated(_, name) => Derived::Explicit(self.db.collation_of(name, self.auth)?),
            ValueExpression::Function { args, .. } => self.all_derived(args.iter())?,
            ValueExpression::Case(case) => self.case_derived(case)?,
            ValueExpression::Subquery(s) => self.subquery_derived(s).into_iter().next().unwrap_or(Derived::Yielding),
            ValueExpression::SetFunction(f) => match &f.arg {
                SetArg::Distinct(argument) | SetArg::All(argument) => self.derived(argument)?,
                SetArg::CountStar => Derived::Yielding,
            },
            ValueExpression::Dyadic(..)
            | ValueExpression::Boolean(_)
            | ValueExpression::CollationFor(_)
            | ValueExpression::Value(_)
            | ValueExpression::Call { .. }
            | ValueExpression::Cast { .. }
            | ValueExpression::Datetime(_)
            | ValueExpression::Extract { .. }
            | ValueExpression::Difference { .. }
            | ValueExpression::AtZone { .. } => Derived::Yielding,
        })
    }

    /// What an operation of `operands` has, none being a value of no
    /// collation of its own.
    fn all_derived<'e>(&self, operands: impl Iterator<Item = &'e ValueExpression>) -> Result<Derived> {
        operands.map(|e| self.derived(e)).try_fold(Derived::Yielding, |a, b| combined(a, b?))
    }

    /// [§9.5] What a CASE's results have.
    fn case_derived(&self, case: &CaseExpression) -> Result<Derived> {
        match case {
            CaseExpression::NullIf(first, _) => self.derived(first),
            CaseExpression::Coalesce(values) => self.all_derived(values.iter()),
            CaseExpression::Simple { whens, otherwise, .. } => {
                self.all_derived(whens.iter().filter_map(|(_, r)| r.as_ref()).chain(otherwise.as_ref()))
            }
            CaseExpression::Searched { whens, otherwise } => {
                self.all_derived(whens.iter().filter_map(|(_, r)| r.as_ref()).chain(otherwise.as_ref()))
            }
        }
    }

    /// Of column `cs`, of this query or an enclosing one, its collation.
    fn column_derived(&self, cs: &ColumnSpecification) -> Result<Derived> {
        let here = match &cs.qualifier {
            Some(q) => self.items.iter().any(|it| it.designated_by(q, self.schema)),
            None => self.visible_has(&cs.column),
        };
        if here {
            let (_, _, column) = match &cs.qualifier {
                Some(q) => self.qualified(q, &cs.column)?,
                None => self.unqualified(&cs.column)?,
            };
            return Ok(Derived::Implicit(column.collation.clone()));
        }
        self.outer.map_or(Ok(Derived::Yielding), |outer| outer.binder().column_derived(cs))
    }

    /// Of each column of `s`, bound already, its collation: one with none
    /// of its own, a literal's, yields.
    fn subquery_derived(&self, s: &AstSubquery) -> Vec<Derived> {
        let key: *const AstSubquery = s;
        let found = self.subquery_collations.borrow();
        found.iter().find(|(at, _)| std::ptr::eq(*at, key)).map_or_else(Vec::new, |(_, names)| {
            names.iter().map(|n| n.clone().map_or(Derived::Yielding, |n| Derived::Implicit(Some(n)))).collect()
        })
    }

    /// Notes the collations of the columns of `s`.
    pub(super) fn note_subquery(&self, s: &AstSubquery, columns: &[OutColumn]) {
        self.subquery_collations.borrow_mut().push((s, columns.iter().map(|c| c.collation.clone()).collect()));
    }

    /// Of each value of `row`, how it came by its collation.
    fn row_derived(&self, row: &[ValueExpression]) -> Result<Vec<Derived>> {
        if let [ValueExpression::Subquery(s)] = row {
            return Ok(self.subquery_derived(s));
        }
        row.iter().map(|e| self.derived(e)).collect()
    }

    /// The collation of an operation on `operands`.
    pub(super) fn collation_of(&self, operands: &[Derived]) -> Result<Option<CollationRef>> {
        determined(self.db, self.session_collation, operands)
    }

    /// The collation an explicit `collate`, or else a column's `implicit`,
    /// gives.
    pub(super) fn chosen(&self, collate: Option<&TableName>, implicit: Derived) -> Result<Option<CollationRef>> {
        let derived = collate.map_or(Ok(implicit), |name| self.db.collation_of(name, self.auth).map(Derived::Explicit))?;
        self.collation_of(&[derived])
    }

    /// `x` and `y`, rows `left` and `right` of one degree compared, each
    /// pair of character strings by the sort keys of its collation.
    pub(super) fn keyed_rows(
        &self,
        (left, right): (&[ValueExpression], &[ValueExpression]),
        x: &BoundRow,
        y: &BoundRow,
    ) -> Result<(BoundRow, BoundRow)> {
        // Only strings have a collation, and most comparisons are of none.
        if !x.iter().zip(y).any(|((_, t), (_, u))| t.is_character() && u.is_character()) {
            return Ok((x.clone(), y.clone()));
        }
        let (of_left, of_right) = (self.row_derived(left)?, self.row_derived(right)?);
        let (mut keyed_x, mut keyed_y) = (x.clone(), y.clone());
        for (i, ((a, t), (b, u))) in keyed_x.iter_mut().zip(keyed_y.iter_mut()).enumerate() {
            if !(t.is_character() && u.is_character()) {
                continue;
            }
            let operands: Vec<Derived> = [of_left.get(i), of_right.get(i)].into_iter().flatten().cloned().collect();
            if let Some(c) = self.collation_of(&operands)? {
                *a = Expr::sort_key(a.clone(), &c);
                *b = Expr::sort_key(b.clone(), &c);
            }
        }
        Ok((keyed_x, keyed_y))
    }

    /// `x`, row `value`, and subquery `s` of column types `types`, compared
    /// by a quantified comparison, MATCH or IN: the pairs of character
    /// strings by the sort keys of their collations.
    pub(super) fn keyed_subquery(
        &self,
        (value, s): (&RowValue, &AstSubquery),
        (x, types): (&BoundRow, &[DataType]),
        bound: &Arc<Subquery>,
    ) -> Result<(BoundRow, Arc<Subquery>)> {
        let of_value = self.row_derived(value)?;
        let of_rows = self.subquery_derived(s);
        let mut keyed = x.clone();
        let mut collations = Vec::with_capacity(types.len());
        for (i, ((e, t), u)) in keyed.iter_mut().zip(types).enumerate() {
            let operands: Vec<Derived> = [of_value.get(i), of_rows.get(i)].into_iter().flatten().cloned().collect();
            let c = if t.is_character() && u.is_character() { self.collation_of(&operands)? } else { None };
            if let Some(c) = &c {
                *e = Expr::sort_key(e.clone(), c);
            }
            collations.push(c);
        }
        let collations = any_of(collations);
        Ok((keyed, if collations.is_empty() { Arc::clone(bound) } else { keyed_subquery(bound, &collations) }))
    }

    /// [ISO/IEC 9075-2:2023 §8.10] `s`, of column types `types`, for UNIQUE,
    /// which compares its rows.
    pub(super) fn keyed_rows_of(&self, s: &AstSubquery, types: &[DataType], bound: &Arc<Subquery>) -> Result<Arc<Subquery>> {
        let of_rows = self.subquery_derived(s);
        let collations = types
            .iter()
            .enumerate()
            .map(|(i, t)| if t.is_character() { self.collation_of(of_rows.get(i).map_or(&[], std::slice::from_ref)) } else { Ok(None) })
            .collect::<Result<Vec<_>>>()?;
        let collations = any_of(collations);
        Ok(if collations.is_empty() { Arc::clone(bound) } else { keyed_subquery(bound, &collations) })
    }

    /// [ISO/IEC 9075-2:2023 §10.7; ISO/IEC 9075:1992 §6.13 SR1] `e`, of a
    /// `<collate clause>`: a character string, whose comparisons take the
    /// collation (`derived`).
    pub(super) fn collated_value(&self, e: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let bound = self.value_expression(e, scope)?;
        if !bound.1.is_character() {
            return Err(rule(format!("[ISO/IEC 9075:1992 §6.13 SR1] a <collate clause> is of a character string, not {}", bound.1)));
        }
        Ok(bound)
    }

    /// [ISO/IEC 9075-2:2023 §6.4 GR13] COLLATION FOR (`e`): the name of `e`'s
    /// own collation, as COLLATE takes it, whatever the session's. Of one
    /// of no collation, derivation none, the null value.
    pub(super) fn collation_for(&self, e: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (_, t) = self.value_expression(e, scope)?;
        if !t.is_character() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.4 SR5] COLLATION FOR is of a character string, not {t}")));
        }
        let named = |name: Option<QualifiedName>| match name {
            Some(n) if n.auth == INFORMATION_SCHEMA => Value::Char(n.ident),
            Some(n) => Value::Char(n.to_string()),
            None => Value::Char(SQL_TEXT.to_owned()),
        };
        let value = match self.derived(e)? {
            Derived::Explicit(name) => named(Some(name)),
            Derived::Implicit(name) => named(name),
            Derived::Yielding => named(None),
            Derived::Conflicting => Value::Null,
        };
        Ok((Expr::Const(value), DataType::Varying { length: MAX_IDENTIFIER_LENGTH }))
    }

    /// The collation of the select-list item `e`, as a result column's.
    pub(super) fn result_collation(&self, e: &ValueExpression) -> Option<QualifiedName> {
        match self.derived(e) {
            Ok(Derived::Implicit(name)) => name,
            Ok(Derived::Explicit(name)) => Some(name),
            Ok(Derived::Yielding | Derived::Conflicting) | Err(_) => None,
        }
    }
}

/// [X3.135-1986 §8.3] `bound` sorted by `keys`, each by its collation in
/// `collations`, if any: the sort keys of a collation are made columns of
/// the rows sorted, and taken away after.
pub(super) fn with_sort(bound: Bound, (keys, collations): (Vec<SortKey>, Vec<Option<CollationRef>>)) -> Bound {
    if keys.is_empty() {
        return bound;
    }
    if collations.iter().all(Option::is_none) {
        return Bound { plan: LogicalPlan::Sort { keys, input: Box::new(bound.plan) }, columns: bound.columns };
    }
    let width = bound.columns.len();
    let mut exprs: Vec<Expr> = (0..width).map(Expr::Column).collect();
    let mut sorted = Vec::with_capacity(keys.len());
    for (key, collation) in keys.into_iter().zip(&collations) {
        sorted.push(match collation {
            Some(c) => {
                exprs.push(Expr::sort_key(Expr::Column(key.column), c));
                SortKey { column: exprs.len().saturating_sub(1), ..key }
            }
            None => key,
        });
    }
    let keyed = LogicalPlan::Project { exprs, input: Box::new(bound.plan) };
    let plan = LogicalPlan::Sort { keys: sorted, input: Box::new(keyed) };
    let kept = (0..width).map(Expr::Column).collect();
    Bound { plan: LogicalPlan::Project { exprs: kept, input: Box::new(plan) }, columns: bound.columns }
}

/// The collation each of the `<sort specification>`s `order_by` sorts by,
/// of the columns `keys` found among `columns`.
pub(super) fn sort_collations(
    (db, auth, session): (&State, &str, Option<&CollationRef>),
    order_by: &[SortSpecification],
    keys: &[SortKey],
    columns: &[OutColumn],
) -> Result<Vec<Option<CollationRef>>> {
    order_by
        .iter()
        .zip(keys)
        .map(|(s, key)| {
            let Some(column) = columns.get(key.column).filter(|c| c.data_type.is_character()) else { return Ok(None) };
            let derived = match &s.collate {
                Some(name) => Derived::Explicit(db.collation_of(name, auth)?),
                None => Derived::Implicit(column.collation.clone()),
            };
            determined(db, session, &[derived])
        })
        .collect()
}
