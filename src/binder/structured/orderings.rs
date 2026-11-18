//! [ISO/IEC 9075-2:2023 §4.13, §9.10 to §9.14, §11.65] Values of
//! user-defined types compared by their types' orderings, bound.
//!
//! A type of no ordering is UDT-NC-ordered, one of EQUALS ONLY UDT-EC-
//! ordered [§4.2.4]: neither is an operand of a grouping or an ordering
//! operation, nor the first of an equality operation. Of a structured type
//! ordered by MAP, each operand is given its map function's value as its
//! comparison key (`functions::Keyed`), so comparing, grouping and sorting
//! it compare that. Ordered by RELATIVE, `x op y` is `RF(x, y) op 0`. By
//! STATE, values compare by most specific type and attributes, as the
//! values themselves do (RULES.md D81).
//!
//! A type of a family built in (`value/types/family.rs`) is ordered as
//! its family says: the geometry types EQUALS ONLY BY RELATIVE with
//! ST_OrderingEquals [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.65]
//! (RULES.md D-SPATIAL-FAMILY), so they are compared for equality as
//! `RF(x, y) = 0`, and neither grouped nor sorted.

use std::sync::Arc;

use super::super::{Binder, rule};
use super::call;
use crate::ast::OrderingForm;
use crate::catalog::{OrderingCategory, OrderingDesc, QualifiedName, SqlRoutineDesc, udt_qualified};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::functions::Keyed;
use crate::logical::{Expr, LogicalPlan, Subquery};
use crate::value::types::family::family_of;
use crate::value::{DataType, Decimal, Value};

/// What an operation needs of its operands' orderings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::binder) enum Need {
    /// An equality operation [§9.10]: a comparison form.
    Equality,
    /// A grouping operation [§9.12]: FULL.
    Grouping,
    /// An ordering operation [§9.14]: FULL, and no reference.
    Ordering,
}

impl Need {
    /// The rule an operand it refuses breaks.
    const fn rule(self) -> &'static str {
        match self {
            Need::Equality => "§9.10 SR3",
            Need::Grouping => "§9.12 SR3",
            Need::Ordering => "§9.14 SR3",
        }
    }
}

/// The user-defined type `t` is, its name and ordering in `db`: of a
/// structured type, its maximal supertype's [§4.13]; none of another type.
fn ordering_of<'d>(db: &'d State, t: &DataType) -> Option<(QualifiedName, Option<&'d OrderingDesc>)> {
    let qn = match t {
        DataType::Structured(s) => {
            let top = s.supertypes.last().unwrap_or(&s.name);
            udt_qualified(top)
        }
        DataType::Distinct(d) => QualifiedName { auth: d.schema.clone(), ident: d.name.clone() },
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Row(_)
        | DataType::Reference(_)
        | DataType::Json
        | DataType::Extension(_) => return None,
    };
    let ordering = db.catalog.ordering(&qn);
    Some((qn, ordering))
}

/// [ISO/IEC 9075-2:2023 §9.10 SR3, §9.12 SR3, §9.14 SR3] That `t`, an
/// operand of `what`, is neither UDT-NC-ordered nor, unless `need` is an
/// equality operation's, UDT-EC-ordered; nor reference-ordered of an
/// ordering operation.
///
/// # Errors
///
/// SQLCODE -103 if it is.
pub(in crate::binder) fn udt_operand(db: &State, t: &DataType, need: Need, what: &str) -> Result<()> {
    let mut work = vec![t];
    while let Some(u) = work.pop() {
        if let DataType::Array(a) = u {
            work.push(&a.element);
        } else if let DataType::Multiset(m) = u {
            work.push(&m.element);
        } else if let DataType::Row(r) = u {
            work.extend(r.fields.iter().map(|f| &f.data_type));
        } else if matches!(u, DataType::Reference(_)) && need == Need::Ordering {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §9.14 SR3] {what} orders values of {u}, and references are compared for equality alone"
            )));
        }
        if let Some((f, kind)) = family_of(u)
            && need != Need::Equality
        {
            // [§14.1.7, §17.2.20] ST_SpatialRefSys's and ST_Vector's, else
            // the geometry types'.
            let clause = crate::geometry::support::of_data_type(u).map_or("§5.1.65", |s| s.ordering_clause());
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 {}] {what} compares values of {}, which has an ordering of EQUALS ONLY (ISO/IEC 13249-3:2016 (DIS, SC32 N2593) {clause}, {})",
                need.rule(),
                kind.name(),
                f.relative_ordering()
            )));
        }
        let Some((qn, ordering)) = ordering_of(db, u) else { continue };
        let refused = match ordering.map(|o| o.form) {
            None => Some("no ordering"),
            Some(OrderingForm::Equals) if need != Need::Equality => Some("an ordering of EQUALS ONLY"),
            Some(OrderingForm::Equals | OrderingForm::Full) => None,
        };
        if let Some(why) = refused {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 {}] {what} compares values of {qn}, which has {why}", need.rule())));
        }
    }
    Ok(())
}

/// Of a comparison of a row and a subquery, the row's values and the
/// subquery, each of a structured type given its comparison key.
type Compared = (Vec<(Expr, DataType)>, Arc<Subquery>);

impl Binder<'_> {
    /// `bound`, an operand of an operation that compares as `need` says,
    /// checked (`udt_operand`) and, of a structured type ordered by MAP,
    /// given its comparison key.
    ///
    /// # Errors
    ///
    /// As `udt_operand`'s; SQLCODE -900 of a structured type ordered by
    /// RELATIVE in a grouping or ordering operation other than a
    /// comparison, whose values have no key of their own (RULES.md D81).
    pub(in crate::binder) fn compared_as(&self, (e, t): (Expr, DataType), need: Need, what: &str) -> Result<(Expr, DataType)> {
        udt_operand(self.db, &t, need, what)?;
        // [§9.11] a multiset compared by its elements, of their keys.
        if t.multiset_type().is_some() {
            return Ok((self.keyed_elements(e, &t, what)?, t));
        }
        let Some((_, Some(o))) = ordering_of(self.db, &t).filter(|_| matches!(t, DataType::Structured(_))) else { return Ok((e, t)) };
        match &o.category {
            OrderingCategory::Map(f) => {
                let key = self.ordering_call(f, vec![e.clone()])?;
                Ok((call(Keyed { result: t.clone() }, vec![e, key]), t))
            }
            OrderingCategory::State(_) => Ok((e, t)),
            OrderingCategory::Relative(_) if need == Need::Equality => Ok((e, t)),
            // Valid SQL [§9.12 SR3, §9.14 SR3]: a sort or a grouping of
            // values compared by a function alone is not built.
            OrderingCategory::Relative(_) => Err(SqlError::new(
                Code::Unsupported,
                format!(
                    "[DESIGN] {what} of {t}, ordered by RELATIVE, is valid SQL this implementation does not take: its values have no key to sort or group by (RULES.md D81)"
                ),
            )),
        }
    }

    /// [ISO/IEC 9075-2:2023 §8.2 GR] `x` and `y` compared: of a structured
    /// type ordered by RELATIVE, `RF(x, y)` and 0, so any comparison of the
    /// two is of `RF(x, y)` with 0; else each as `compared_as` gives it.
    ///
    /// # Errors
    ///
    /// As `compared_as`'s.
    pub(in crate::binder) fn compared_pair(
        &self,
        x: (Expr, DataType),
        y: (Expr, DataType),
        need: Need,
    ) -> Result<((Expr, DataType), (Expr, DataType))> {
        if family_of(&x.1).is_some() {
            udt_operand(self.db, &x.1, need, "the comparison")?;
            udt_operand(self.db, &y.1, need, "the comparison")?;
            let integer = super::super::composite::integer();
            let relative = call(crate::geometry::OrderingEquals, vec![x.0, y.0]);
            return Ok(((relative, integer.clone()), (Expr::Const(Value::Exact(Decimal::new(0_i128, 0_u32))), integer)));
        }
        if let Some((_, Some(o))) = ordering_of(self.db, &x.1).filter(|_| matches!(x.1, DataType::Structured(_)))
            && let OrderingCategory::Relative(f) = &o.category
        {
            udt_operand(self.db, &x.1, need, "the comparison")?;
            let integer = super::super::composite::integer();
            let relative = self.ordering_call(f, vec![x.0, y.0])?;
            return Ok(((relative, integer.clone()), (Expr::Const(Value::Exact(Decimal::new(0_i128, 0_u32))), integer)));
        }
        Ok((self.compared_as(x, need, "the comparison")?, self.compared_as(y, need, "the comparison")?))
    }

    /// [ISO/IEC 9075-2:2023 §8.2 GR, §8.7, §8.10] `x`, a row compared by
    /// `need` with each row of `s`, of column types `types`, and `s`, each
    /// pair of user-defined type as its ordering compares it: of MAP, both
    /// by their keys; of RELATIVE, 0 in `x` and `RF(x, y)` in `s`, `x`
    /// reaching it as an outer value; of STATE, or of another type, as
    /// they are.
    ///
    /// # Errors
    ///
    /// As `compared_as`'s.
    pub(in crate::binder) fn udt_subquery(
        &self,
        x: Vec<(Expr, DataType)>,
        types: &[DataType],
        (s, need): (Arc<Subquery>, Need),
    ) -> Result<Compared> {
        if !types.iter().any(|t| matches!(t, DataType::Structured(_) | DataType::Multiset(_))) {
            return Ok((x, s));
        }
        let mut outer = s.outer.clone();
        let mut columns: Vec<Expr> = (0..types.len()).map(Expr::Column).collect();
        let mut row = Vec::with_capacity(x.len());
        for (i, ((e, t), u)) in x.into_iter().zip(types).enumerate() {
            let relative = ordering_of(self.db, u)
                .filter(|_| matches!(u, DataType::Structured(_)))
                .and_then(|(_, o)| o)
                .and_then(|o| if let OrderingCategory::Relative(f) = &o.category { Some(f.clone()) } else { None });
            let Some(slot) = columns.get_mut(i) else { continue };
            if let Some(f) = relative {
                udt_operand(self.db, u, need, "the comparison")?;
                let at = outer.len();
                outer.push(e);
                *slot = self.ordering_call(&f, vec![Expr::Outer(at, String::new()), Expr::Column(i)])?;
                row.push((Expr::Const(Value::Exact(Decimal::new(0_i128, 0_u32))), super::super::composite::integer()));
                continue;
            }
            *slot = self.compared_as((Expr::Column(i), u.clone()), need, "the comparison")?.0;
            row.push(self.compared_as((e, t), need, "the comparison")?);
        }
        let plan = LogicalPlan::Project { exprs: columns, input: Box::new(s.plan.clone()) };
        Ok((row, Arc::new(Subquery::new(plan, outer))))
    }

    /// The invocation of the ordering function of specific name `f` of
    /// `args`.
    fn ordering_call(&self, f: &QualifiedName, args: Vec<Expr>) -> Result<Expr> {
        let desc: &SqlRoutineDesc =
            self.db.catalog.routine(f).ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §11.65] no ordering function {f}")))?;
        Ok(Expr::Invoked(Arc::new(self.bound_invocation(desc, args)?)))
    }
}

/// [ISO/IEC 9075-2:2023 §9.12, §9.14] `e`, of type `t`, an operand of the
/// grouping or ordering operation `what` bound in `cx`: of a structured
/// type ordered by MAP, given its comparison key, so the operation
/// compares that; else `e`.
///
/// # Errors
///
/// SQLCODE -900 of a structured type ordered by RELATIVE, whose values
/// have no key of their own (RULES.md D81).
pub(in crate::binder) fn keyed_operand(cx: super::super::Context<'_>, (e, t): (Expr, &DataType), what: &str) -> Result<Expr> {
    if !matches!(t, DataType::Structured(_)) {
        return Ok(e);
    }
    let binder = Binder::new(cx, None, (Vec::new(), Vec::new(), Vec::new()), std::collections::BTreeSet::new());
    binder.compared_as((e, t.clone()), Need::Grouping, what).map(|(e, _)| e)
}

/// `plan`, its columns `types`, each of a structured type ordered by MAP
/// given its comparison key (`keyed_operand`), so the grouping operation
/// `what` over them compares that; `plan` itself if none is.
///
/// # Errors
///
/// As `keyed_operand`'s.
pub(in crate::binder) fn keyed_columns(
    cx: super::super::Context<'_>,
    plan: LogicalPlan,
    types: &[DataType],
    what: &str,
) -> Result<LogicalPlan> {
    if !types.iter().any(|t| matches!(t, DataType::Structured(_))) {
        return Ok(plan);
    }
    let exprs = types.iter().enumerate().map(|(i, t)| keyed_operand(cx, (Expr::Column(i), t), what)).collect::<Result<Vec<Expr>>>()?;
    Ok(LogicalPlan::Project { exprs, input: Box::new(plan) })
}

/// Whether `t` is a structured type ordered by MAP in `db`, whose values a
/// grouping or ordering operation compares by their keys.
pub(in crate::binder) fn mapped(db: &State, t: &DataType) -> bool {
    matches!(t, DataType::Structured(_))
        && matches!(ordering_of(db, t), Some((_, Some(o))) if matches!(o.category, OrderingCategory::Map(_)))
}
