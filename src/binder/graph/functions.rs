//! \[EXTENSION\] The element and path functions of a GRAPH_TABLE [GS §5,
//! §6.9] and the label expressions of its label predicates [GS §6.2]:
//! ELEMENT_ID, VERTEX_ID and EDGE_ID of an element's identity, LABELS of
//! its element table's labels, and the types of the path functions'
//! values, computed of each match (`exec/graph_table/values.rs`).

use std::sync::Arc;

use super::super::rule;
use super::scope::{Graphing, ID, TABLE, table_number};
use crate::ast::{GraphFunction, GraphFunctionName, LabelExpression};
use crate::error::Result;
use crate::functions::ScalarRef;
use crate::logical::{Cond, Expr, GraphValue, IdentityJson, PathFunction, identity_type};
use crate::value::{CharUnits, CompOp, DataType, Decimal, Value};

/// \[DESIGN\] The most elements of a path, or labels of an element, an
/// array of them holds: the most an array may hold.
const ARRAY_MOST: u32 = crate::value::types::MAX_ARRAY_CARDINALITY;

/// \[DESIGN\] The longest label name, in characters: an identifier's.
const LABEL_LENGTH: u32 = 128;

/// The type of a label's name.
fn label_type() -> DataType {
    DataType::Varying { length: LABEL_LENGTH, units: CharUnits::Characters }
}

/// \[GS §6.9\] The type of path function `f`'s value.
pub(super) fn path_type(f: PathFunction) -> DataType {
    match f {
        PathFunction::Length => super::super::composite::integer(),
        PathFunction::Vertices | PathFunction::Edges | PathFunction::Elements => DataType::array(identity_type(), ARRAY_MOST),
        PathFunction::Acyclic | PathFunction::Trail | PathFunction::Simple => DataType::Boolean,
        PathFunction::Json => DataType::Json,
    }
}

/// \[GS §11.1\] PREV or FIRST, as `f` writes it, a slot of a prefilter:
/// the identity of the element of its variable, or the property named.
///
/// # Errors
///
/// SQLCODE -103 outside a prefilter, of no element variable, or of a
/// property the variable has none of (GRF-0012).
pub(super) fn iteration(g: &Graphing, f: &GraphFunction) -> Result<(Expr, DataType)> {
    if !g.filtering.get() {
        return Err(rule("[GS §11.1] PREV and FIRST are written in a prefilter, of an iteration of a quantified pattern"));
    }
    let var = g.var_named(&f.operand.name)?;
    let columns = g.vars.get(var).map(|v| v.columns.as_slice()).unwrap_or_default();
    let (property, t) = match &f.property {
        Some(name) => {
            let k = columns
                .iter()
                .position(|(n, _)| n == name)
                .ok_or_else(|| super::grf("GRF-0012", format!("{} has no property {name}", f.operand.name)))?;
            (Some(k), columns.get(k).map_or(DataType::Boolean, |c| c.1.clone()))
        }
        None => (None, identity_type()),
    };
    let first = f.name == GraphFunctionName::First;
    Ok((g.slot(GraphValue::Iteration { var, first, property }), t))
}

/// \[GS §6.2\] Whether an element of labels `labels` satisfies `e`.
pub(super) fn label_holds(e: &LabelExpression, labels: &[String]) -> bool {
    match e {
        LabelExpression::Label(name, _) => labels.iter().any(|l| l == name),
        LabelExpression::Wildcard(_) => !labels.is_empty(),
        LabelExpression::Negation(inner) => !label_holds(inner, labels),
        LabelExpression::Conjunction(a, b) => label_holds(a, labels) && label_holds(b, labels),
        LabelExpression::Disjunction(a, b) => label_holds(a, labels) || label_holds(b, labels),
    }
}

/// The comparison of an element table's number, of a hidden column, with
/// that of `t`.
fn is_table(column: &Expr, t: usize) -> Cond {
    let number = Value::Exact(Decimal::new(i128::from(table_number(t)), 0));
    Cond::Compare(column.clone(), CompOp::Eq, Expr::Const(number))
}

/// \[GS §5 R5.1, R5.4\] ELEMENT_ID, VERTEX_ID, EDGE_ID or LABELS of the
/// element variable `v` of `g`.
///
/// # Errors
///
/// SQLCODE -103 if `v` is of the wrong kind for VERTEX_ID or EDGE_ID.
pub(super) fn element_function(
    binder: &super::super::Binder<'_>,
    g: &Graphing,
    (name, v): (GraphFunctionName, usize),
) -> Result<(Expr, DataType)> {
    let var = g.vars.get(v).ok_or_else(|| rule("[DESIGN] a variable is gone"))?;
    let id = binder.graph_column(var.hidden(ID), super::super::Scope::Select).unwrap_or(Expr::Column(var.hidden(ID)));
    match name {
        GraphFunctionName::VertexId | GraphFunctionName::EdgeId => {
            if (name == GraphFunctionName::EdgeId) != var.edge {
                let what = if var.edge { "an edge" } else { "a vertex" };
                return Err(rule(format!("[GS §5 R5.1] {} of {}, {what}", name.word(), var.name)));
            }
            Ok((Expr::Call(ScalarRef(Arc::new(IdentityJson)), vec![id]), DataType::Json))
        }
        GraphFunctionName::Labels => {
            let table = binder.graph_column(var.hidden(TABLE), super::super::Scope::Select).unwrap_or(Expr::Column(var.hidden(TABLE)));
            let whens = var
                .tables
                .iter()
                .map(|(t, labels, _)| {
                    let names: Vec<Value> = labels.iter().map(|l| Value::Char(l.clone())).collect();
                    (is_table(&table, *t), Expr::Const(Value::Array(names.into())))
                })
                .collect();
            Ok((Expr::Case(whens, Box::new(Expr::Const(Value::Null))), DataType::array(label_type(), ARRAY_MOST)))
        }
        GraphFunctionName::ElementId
        | GraphFunctionName::PathLength
        | GraphFunctionName::Vertices
        | GraphFunctionName::Edges
        | GraphFunctionName::IsAcyclic
        | GraphFunctionName::IsTrail
        | GraphFunctionName::IsSimple
        | GraphFunctionName::PathToJson
        | GraphFunctionName::Prev
        | GraphFunctionName::First => Ok((id, identity_type())),
    }
}
