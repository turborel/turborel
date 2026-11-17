//! Multisets bound [ISO/IEC 9075-2:2023 §4.12.3]: their constructors by
//! enumeration [§6.52], MULTISET UNION, EXCEPT and INTERSECT [§6.50], SET
//! [§6.51], ELEMENT [§6.26], the member, submultiset and set predicates
//! [§8.16 to §8.18] and COLLECT, FUSION and INTERSECTION [§10.9], each a
//! call of a function of `functions/multisets.rs` (RULES.md D68, D80);
//! and the rules of multiset element grouping operations [§9.13] and of
//! the sources of non-determinism among them [§9.16].

use std::sync::Arc;

use super::case::{brought, set_type};
use super::composite::bounded;
use super::expression::{dynamic_of, nondeterministic_source};
use super::{Binder, Scope, rule};
use crate::ast::{Composite, RowValue, SetArg, SetFunctionSpecification, ValueExpression};
use crate::error::{Code, Result, SqlError};
use crate::functions::{
    AggregateRef, ElementOfMultiset, Gathering, IsASet, MemberOf, MultisetAggregate, MultisetCombination, MultisetOf, ScalarRef, SetOf,
    SubmultisetOf,
};
use crate::logical::{Cond, Expr};
use crate::params::ParameterType;
use crate::value::DataType;
use crate::value::multisets::BagOp;
use crate::value::types::MAX_MULTISET_CARDINALITY;
use crate::value::types::composite::unordered;

/// The call of `f`, of `args`.
fn call(f: impl crate::functions::ScalarFunction + 'static, args: Vec<Expr>) -> Expr {
    Expr::Call(ScalarRef(Arc::new(f)), args)
}

/// [ISO/IEC 9075-2:2023 §9.13 SR3] A multiset operand of a multiset
/// element grouping operation, `what`, of element type `element`: not
/// LOB-ordered, array-ordered or multiset-ordered.
///
/// # Errors
///
/// SQLCODE -103 if it is.
pub(super) fn grouped_elements(element: &DataType, what: &str) -> Result<()> {
    if let Some(these) = unordered(element) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §9.13 SR3] {what} groups elements of {element}, and {these} are grouped by no operation"
        )));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §9.13 SR1a, SR3] Of an operand of type `t` of an
/// equality operation, `what`: if it is a multiset, its elements grouped
/// by no operation refused, as `grouped_elements` refuses them.
///
/// # Errors
///
/// SQLCODE -103 if they are.
pub(super) fn equality_operand(t: &DataType, what: &str) -> Result<()> {
    match t.multiset_type() {
        Some(m) => grouped_elements(&m.element, what),
        None => Ok(()),
    }
}

/// Whether `t` is based on a character string type, a user-defined type
/// or a datetime type with time zone [ISO/IEC 9075-2:2023 §4.2]: so its
/// values that are not distinct may yet differ [§9.16 SR1n, SR1o, SR1aq].
fn uncertain(t: &DataType) -> bool {
    match t {
        DataType::Character { .. } | DataType::Varying { .. } | DataType::Distinct(_) | DataType::Structured(_) => true,
        DataType::Lob { .. } => t.is_character(),
        DataType::Time { zoned, .. } | DataType::Timestamp { zoned, .. } => *zoned,
        DataType::Array(a) => uncertain(&a.element),
        DataType::Multiset(m) => uncertain(&m.element),
        DataType::Row(r) => r.fields.iter().any(|f| uncertain(&f.data_type)),
        DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Interval(_)
        | DataType::Reference(_)
        | DataType::Extension(_) => false,
    }
}

/// Whether `a` and `b` have corresponding constituents of which one is a
/// datetime with time zone and the other a datetime without [ISO/IEC
/// 9075-2:2023 §9.16 SR1m, SR1aj, SR1ak].
fn zones_mixed(a: &DataType, b: &DataType) -> bool {
    let zoned = |t: &DataType| {
        if let DataType::Time { zoned, .. } | DataType::Timestamp { zoned, .. } = t { Some(*zoned) } else { None }
    };
    if let (Some(x), Some(y)) = (zoned(a), zoned(b)) {
        return x != y;
    }
    let (a, b) = (a.represented(), b.represented());
    if let (DataType::Array(x), DataType::Array(y)) = (a, b) {
        return zones_mixed(&x.element, &y.element);
    }
    if let (DataType::Multiset(x), DataType::Multiset(y)) = (a, b) {
        return zones_mixed(&x.element, &y.element);
    }
    if let (DataType::Row(x), DataType::Row(y)) = (a, b) {
        return x.fields.iter().zip(&y.fields).any(|(f, g)| zones_mixed(&f.data_type, &g.data_type));
    }
    false
}

impl Binder<'_> {
    /// Of a constraint, `what`, a potential source of non-determinism by
    /// the item `item` of §9.16 SR1, refused if `uncertain` holds.
    fn certain(&self, uncertain: bool, (item, what): (&str, &str)) -> Result<()> {
        if uncertain && self.purpose.constrains() {
            return Err(nondeterministic_source(item, what));
        }
        Ok(())
    }

    /// `v`, a `<multiset value expression>` of `what`, bound: its value
    /// and type, and its element type [§6.50 SR1]; a `?` taking the type
    /// `taken` if given.
    fn multiset_operand(
        &self,
        v: &ValueExpression,
        taken: Option<&DataType>,
        what: &str,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType, DataType)> {
        let (e, t) = match taken {
            Some(t) => self.typed(v, ParameterType::Of(t.clone()), scope)?,
            None => self.value_expression(v, scope)?,
        };
        let Some(m) = t.multiset_type() else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.50 SR1] the operand of {what} is a multiset, not of {t}")));
        };
        let element = m.element.clone();
        Ok((e, t, element))
    }

    /// [ISO/IEC 9075-2:2023 §6.52 SR3, GR2] `<multiset value constructor
    /// by enumeration>`: of the type of all its elements [§9.5], each cast
    /// to it. MULTISET[] has a type only where its context gives it one
    /// [§6.5 SR1], as `typed` takes it.
    pub(super) fn multiset_of(&self, values: &[ValueExpression], scope: Scope<'_>) -> Result<(Expr, DataType)> {
        if values.is_empty() {
            return Err(rule("[ISO/IEC 9075-2:2023 §6.5 SR1] MULTISET[] is of the element type its context gives, and none is given here"));
        }
        if u32::try_from(values.len()).map_or(true, |n| n > MAX_MULTISET_CARDINALITY) {
            return Err(SqlError::new(
                Code::Unsupported,
                format!(
                    "[ISO/IEC 9075-2:2023 §6.52 SR3] MULTISET of {} elements exceeds this implementation's limit of {MAX_MULTISET_CARDINALITY}",
                    values.len()
                ),
            ));
        }
        let bound = values.iter().map(|v| self.value_expression(v, scope)).collect::<Result<Vec<_>>>()?;
        let types: Vec<DataType> = bound.iter().map(|(_, t)| t.clone()).collect();
        let element = set_type(&types, "§6.52 SR3")?;
        let args = bound.into_iter().map(|(e, t)| brought(e, &t, element.clone())).collect();
        let t = bounded(DataType::multiset(element))?;
        Ok((call(MultisetOf(t.clone()), args), t))
    }

    /// [ISO/IEC 9075-2:2023 §6.50 SR2 to SR4] MULTISET UNION, EXCEPT or
    /// INTERSECT of `left` and `right`: of a multiset of the type of both
    /// element types [§9.5], each operand cast to it; all but UNION ALL a
    /// multiset element grouping operation [§9.13], and of a constraint
    /// not a source of non-determinism [§9.16 SR1m, SR1n].
    pub(super) fn multiset_operation(
        &self,
        (op, distinct): (BagOp, bool),
        (left, right): (&ValueExpression, &ValueExpression),
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let spelled = MultisetCombination(op, distinct, DataType::Boolean);
        let what = crate::functions::ScalarFunction::name(&spelled).to_owned();
        let [(first, first_type), (second, second_type)] = self.pair((left, right), scope)?;
        let (Some(of_first), Some(of_second)) = (first_type.multiset_type(), second_type.multiset_type()) else {
            let other = if first_type.multiset_type().is_none() { &first_type } else { &second_type };
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.50 SR1] the operands of {what} are multisets, not of {other}")));
        };
        let grouping = distinct || op != BagOp::Union;
        if grouping {
            grouped_elements(&of_first.element, &what)?;
            grouped_elements(&of_second.element, &what)?;
        }
        let element = set_type(&[of_first.element.clone(), of_second.element.clone()], "§6.50 SR2b, SR3b, SR4b")?;
        let mixed = zones_mixed(&of_first.element, &of_second.element);
        self.certain(mixed, ("m", &format!("{what} of datetimes with and without time zone")))?;
        self.certain(grouping && uncertain(&element), ("n", &format!("{what} of elements of {element}")))?;
        let t = bounded(DataType::multiset(element))?;
        let mut args = vec![brought(first, &first_type, t.clone()), brought(second, &second_type, t.clone())];
        if grouping {
            args = args.into_iter().map(|a| self.keyed_elements(a, &t, &what)).collect::<Result<_>>()?;
        }
        Ok((call(MultisetCombination(op, distinct, t.clone()), args), t))
    }

    /// [ISO/IEC 9075-2:2023 §6.51] `<multiset set function>`: of its
    /// operand's multiset type, not a distinct type's [SR2], a multiset
    /// element grouping operation \[SR1\], and of a constraint not a source
    /// of non-determinism [§9.16 SR1o].
    pub(super) fn multiset_set(&self, value: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (e, t, element) = self.multiset_operand(value, None, "SET", scope)?;
        grouped_elements(&element, "SET")?;
        self.certain(uncertain(&element), ("o", &format!("SET of a multiset of {element}")))?;
        let result = DataType::multiset(element);
        let e = self.keyed_elements(brought(e, &t, result.clone()), &result, "SET")?;
        Ok((call(SetOf(result.clone()), vec![e]), result))
    }

    /// [ISO/IEC 9075-2:2023 §6.26 SR1] `<multiset element reference>`: of
    /// the element type, as the scalar subquery of UNNEST of it is.
    pub(super) fn multiset_element(&self, value: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (e, _, element) = self.multiset_operand(value, None, "ELEMENT", scope)?;
        Ok((call(ElementOfMultiset(element.clone()), vec![e]), element))
    }

    /// [ISO/IEC 9075-2:2023 §8.16] "X MEMBER OF MVE": X a row of one value
    /// or the row its values make \[SR2\], comparable with the element type
    /// \[SR3\], an operand of an equality operation [SR4, §9.11]; a `?` X
    /// taking the element type, a `?` MVE a multiset of X's type.
    pub(super) fn member(&self, value: &RowValue, multiset: &ValueExpression, not: bool, scope: Scope<'_>) -> Result<Cond> {
        let x =
            if let [one] = value.as_slice() { one.clone() } else { ValueExpression::Composite(Box::new(Composite::Row(value.clone())), 0) };
        let (xe, xt, mve, element) = if dynamic_of(&x).is_some() && dynamic_of(multiset).is_none() {
            let (mve, _, element) = self.multiset_operand(multiset, None, "MEMBER", scope)?;
            let (xe, xt) = self.typed(&x, ParameterType::Of(element.clone()), scope)?;
            (xe, xt, mve, element)
        } else {
            let (xe, xt) = self.value_expression(&x, scope)?;
            let (mve, _, element) = self.multiset_operand(multiset, Some(&DataType::multiset(xt.clone())), "MEMBER", scope)?;
            (xe, xt, mve, element)
        };
        if !xt.comparable(&element) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §8.16 SR3] {xt} and the element type {element} are not comparable")));
        }
        equality_operand(&xt, "MEMBER")?;
        self.certain(zones_mixed(&xt, &element), ("aj", "MEMBER of datetimes with and without time zone"))?;
        // [§9.11] X and the elements compared by their keys: X as the one
        // element of MULTISET[X], keyed so.
        let mve = self.keyed_elements(mve, &DataType::multiset(element), "MEMBER")?;
        let xe = self.keyed_value(self.compared_as((xe, xt.clone()), super::structured::Need::Equality, "MEMBER")?.0, &xt, "MEMBER")?;
        Ok(negated(Cond::Value(call(MemberOf, vec![xe, mve])), not))
    }

    /// [ISO/IEC 9075-2:2023 §8.17] "CVE SUBMULTISET OF MVE": CVE one value
    /// of a multiset type \[SR1\], their element types comparable \[SR3\],
    /// both multiset operands of a multiset element grouping operation
    /// \[SR4\]; a `?` taking the other's type.
    pub(super) fn submultiset(&self, value: &RowValue, multiset: &ValueExpression, not: bool, scope: Scope<'_>) -> Result<Cond> {
        let [cve] = value.as_slice() else {
            return Err(rule("[ISO/IEC 9075-2:2023 §8.17 SR1] the operand of SUBMULTISET is one value, not a row of several"));
        };
        let [(c, ct), (m, mt)] = self.pair((cve, multiset), scope)?;
        let (Some(x), Some(y)) = (ct.multiset_type(), mt.multiset_type()) else {
            let other = if ct.multiset_type().is_none() { &ct } else { &mt };
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §8.17 SR1, SR2] the operands of SUBMULTISET are multisets, not of {other}")));
        };
        if !x.element.comparable(&y.element) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §8.17 SR3] the element types {} and {} are not comparable",
                x.element, y.element
            )));
        }
        grouped_elements(&x.element, "SUBMULTISET")?;
        grouped_elements(&y.element, "SUBMULTISET")?;
        self.certain(zones_mixed(&ct, &mt), ("ak", "SUBMULTISET of datetimes with and without time zone"))?;
        let (c, m) = (self.keyed_elements(c, &ct, "SUBMULTISET")?, self.keyed_elements(m, &mt, "SUBMULTISET")?);
        Ok(negated(Cond::Value(call(SubmultisetOf, vec![c, m])), not))
    }

    /// [ISO/IEC 9075-2:2023 §8.18] "CVE IS A SET": CVE one value of a
    /// multiset type \[SR1\], a multiset operand of a multiset element
    /// grouping operation \[SR2\].
    pub(super) fn set_predicate(&self, value: &RowValue, not: bool, scope: Scope<'_>) -> Result<Cond> {
        let [cve] = value.as_slice() else {
            return Err(rule("[ISO/IEC 9075-2:2023 §8.18 SR1] the operand of IS A SET is one value, not a row of several"));
        };
        let (e, t, element) = self.multiset_operand(cve, None, "IS A SET", scope)?;
        grouped_elements(&element, "IS A SET")?;
        let e = self.keyed_elements(e, &t, "IS A SET")?;
        Ok(negated(Cond::Value(call(IsASet, vec![e])), not))
    }

    /// [ISO/IEC 9075-2:2023 §10.9 SR7k to SR7n] COLLECT, FUSION or
    /// INTERSECTION, if `sf` is one: COLLECT ( X ) of the row ( X ), as
    /// FUSION ( MULTISET [ X ] ) keeps a null X \[SR7l\]; FUSION and
    /// INTERSECTION of no DISTINCT \[SR7m, SR7n\], INTERSECTION's operand
    /// a multiset operand of a multiset element grouping operation, and of
    /// a constraint not a source of non-determinism [§9.16 SR1aq].
    pub(super) fn multiset_aggregate(&self, sf: &SetFunctionSpecification, scope: Scope<'_>) -> Option<Result<(Expr, DataType)>> {
        let kind = [Gathering::Collect, Gathering::Fusion, Gathering::Intersection].into_iter().find(|k| k.name() == sf.name)?;
        Some(self.gathered(kind, sf, scope))
    }

    /// `multiset_aggregate`'s work, of `kind`.
    fn gathered(&self, kind: Gathering, sf: &SetFunctionSpecification, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (value, distinct) = match &sf.arg {
            SetArg::All(v) => (v, false),
            SetArg::Distinct(v) => (v, true),
            SetArg::CountStar | SetArg::Grouping(_) | SetArg::Hypothetical(_) | SetArg::Fraction(_) | SetArg::Variable(..) => {
                return Err(SqlError::new(Code::Api, format!("[DESIGN] {} is read with a value", kind.name())));
            }
        };
        if distinct && kind != Gathering::Collect {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.9 SR7m, SR7n] {} takes no DISTINCT", kind.name())));
        }
        let func = AggregateRef(Arc::new(MultisetAggregate { kind, by_query: false }));
        if kind == Gathering::Collect {
            let row = ValueExpression::Composite(Box::new(Composite::Row(vec![(**value).clone()])), sf.at);
            return self.aggregate(func, (Some(&row), distinct), sf.filter.as_deref(), scope);
        }
        let (e, t) = self.aggregate(func, (Some(value), false), sf.filter.as_deref(), scope)?;
        if kind == Gathering::Intersection
            && let Some(m) = t.multiset_type()
        {
            grouped_elements(&m.element, "INTERSECTION")?;
            self.certain(uncertain(&m.element), ("aq", &format!("INTERSECTION of multisets of {}", m.element)))?;
        }
        Ok((e, t))
    }
}

/// `c`, or NOT `c` if `not`.
fn negated(c: Cond, not: bool) -> Cond {
    if not { Cond::Not(Box::new(c)) } else { c }
}
