//! [ISO/IEC 9075-2:2023 §9.11, §9.13] The elements of a multiset of a
//! structured type ordered by MAP, given their comparison keys.
//!
//! A multiset element grouping operation, or an equality operation of
//! multisets, compares elements. Of a type ordered by MAP it compares their
//! map functions' values, so each operand `m` is bound as
//! `CASE WHEN m IS NULL THEN NULL ELSE MULTISET(SELECT K(E) FROM UNNEST(m)
//! AS U(E)) END`, `K(E)` the element given its key (`functions::Keyed`).
//! Of an element of a row type, each field of such a type is keyed so.

use std::sync::Arc;

use super::super::Binder;
use super::super::composite::anonymous;
use super::orderings::{Need, mapped};
use crate::error::Result;
use crate::functions::{AggregateRef, ElementOfMultiset, Gathering, MultisetAggregate, MultisetOf, RowOf, ScalarRef};
use crate::logical::{AggCall, Cond, Expr, LogicalPlan, Subquery, Unnest};
use crate::value::types::Field;
use crate::value::{DataType, Value};

impl Binder<'_> {
    /// `e`, of type `t`, a value compared with others by an operation
    /// `what`: of a row type, each field of a structured type ordered by
    /// MAP given its key, as the one element of `MULTISET[e]` keyed by
    /// `keyed_elements` is; else `e`.
    ///
    /// # Errors
    ///
    /// As `keyed_elements`'s.
    pub(in crate::binder) fn keyed_value(&self, e: Expr, t: &DataType, what: &str) -> Result<Expr> {
        if !matches!(t, DataType::Row(_)) {
            return Ok(e);
        }
        let alone = DataType::multiset(t.clone());
        let keyed = self.keyed_elements(Expr::Call(ScalarRef(Arc::new(MultisetOf(alone.clone()))), vec![e.clone()]), &alone, what)?;
        Ok(if matches!(keyed, Expr::Case(..)) { Expr::Call(ScalarRef(Arc::new(ElementOfMultiset(t.clone()))), vec![keyed]) } else { e })
    }

    /// `e`, a multiset of type `t` compared element by element by an
    /// operation `what`: its elements, or their fields, of a structured
    /// type ordered by MAP given their keys; else `e`.
    ///
    /// # Errors
    ///
    /// As `compared_as`'s of an element.
    pub(in crate::binder) fn keyed_elements(&self, e: Expr, t: &DataType, what: &str) -> Result<Expr> {
        let Some(m) = t.multiset_type() else { return Ok(e) };
        let element = m.element.clone();
        let fields: Vec<DataType> =
            if let DataType::Row(r) = &element { r.fields.iter().map(|f| f.data_type.clone()).collect() } else { vec![element.clone()] };
        if !fields.iter().any(|f| mapped(self.db, f)) {
            return Ok(e);
        }
        let mut keyed = Vec::with_capacity(fields.len());
        for (i, f) in fields.iter().enumerate() {
            keyed.push(self.compared_as((Expr::Column(i), f.clone()), Need::Grouping, what)?.0);
        }
        let spread = if let DataType::Row(r) = &element { Some(r.fields.len()) } else { None };
        let value = if spread.is_some() {
            Expr::Call(ScalarRef(Arc::new(RowOf(element.clone()))), keyed)
        } else {
            keyed.pop().unwrap_or(Expr::Column(0))
        };
        let unnest = Unnest {
            collections: vec![Expr::Outer(0, String::new())],
            spread: vec![spread],
            ordinality: false,
            outer: None,
            names: (0..fields.len()).map(anonymous).collect(),
            input: LogicalPlan::Values { rows: vec![Vec::new()], types: Vec::new() },
        };
        let one = DataType::row(vec![Field { name: anonymous(0), data_type: element, unnamed: true }]);
        let arg = Expr::Call(ScalarRef(Arc::new(RowOf(one))), vec![value]);
        let agg = AggCall {
            func: AggregateRef(Arc::new(MultisetAggregate { kind: Gathering::Collect, by_query: true })),
            arg: Some(arg),
            distinct: false,
            result: t.clone(),
            filter: None,
            distribution: None,
            label: format!("MULTISET({what})"),
        };
        let plan = LogicalPlan::Aggregate {
            groups: Vec::new(),
            collations: Vec::new(),
            aggs: vec![agg],
            seeds: Vec::new(),
            input: Box::new(LogicalPlan::Unnest(Box::new(unnest))),
        };
        let collected = Expr::Subquery(Arc::new(Subquery::new(plan, vec![e.clone()])), 0);
        Ok(Expr::Case(vec![(Cond::IsNull(e, false), Expr::Const(Value::Null))], Box::new(collected)))
    }
}
