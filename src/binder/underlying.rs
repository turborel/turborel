//! [ISO/IEC 9075:1992 §11.19 GR6] What a view's columns are drawn from:
//! the leaf underlying tables of its query, and of each of its columns the
//! columns of them that are its underlying columns, which its owner's
//! REFERENCES on it are found from (`catalog::applicable`). Split from
//! binder/views.rs (RULES.md R-SIZE-2).
//!
//! The leaf underlying tables [§4.9] are the tables and views the `<from
//! clause>`s of the query name, through its joined and derived tables and
//! its set operations' operands: a view has no underlying table [§4.9], so
//! it is a leaf. The underlying columns of a column of a `<query
//! specification>` are those its `<value expression>` references, in a
//! subquery too, and those its `<table expression>` references, in a
//! derived table or a join condition too [§7.9 SR5, §7.3 SR2, §6.4 SR9];
//! of a common column of a NATURAL or USING join, both its join columns',
//! as `COALESCE ( TA.C, TB.C )` [§7.5 SR6d]. A `<table value constructor>`'s
//! have none [§7.10 SR16a]; a set operation's are both operands' [SR16d],
//! with CORRESPONDING those of the columns of each it names [SR9a iv].
//! Of each column only those of a leaf underlying table are kept.

use super::from::FromItem;
use super::query::Nesting;
use super::set_operations::bind_query_expression;
use super::{Binder, Context, Selected, Uses, qualify, views};
use crate::ast::{
    Correspondence, Literal, QueryExpression, QuerySpecification, SelectItem, SelectList, SelectSublist, SetOperation, TableReference,
    ValueExpression, ValueSpecification,
};
use crate::catalog::{QualifiedName, Underlying};
use crate::error::Result;
use crate::value::Decimal;

/// What the columns of the view whose query is `query`, bound with `cx`,
/// are drawn from.
///
/// # Errors
///
/// The query does not bind.
pub fn underlying(cx: Context<'_>, query: &QueryExpression) -> Result<Underlying> {
    match query {
        QueryExpression::Spec(spec) => of_spec(cx, query, spec),
        QueryExpression::Values(_) => {
            let degree = bind_query_expression(cx, query, &Nesting::Query)?.columns.len();
            Ok(Underlying { leaves: Vec::new(), columns: vec![Vec::new(); degree] })
        }
        QueryExpression::SetOp(s) => {
            let (left, right) = (underlying(cx, &s.left)?, underlying(cx, &s.right)?);
            match &s.corresponding {
                Correspondence::Position => Ok(merged(left, right)),
                Correspondence::Names | Correspondence::By(_) => corresponded(cx, s, (left, right)),
            }
        }
    }
}

/// [ISO/IEC 9075:1992 §7.10 SR16d] What a set operation's columns are
/// drawn from, of its operands' `left` and `right`: each the i-th columns'
/// of both, its leaves both's.
fn merged(left: Underlying, right: Underlying) -> Underlying {
    let mut leaves = left.leaves;
    add_new(&mut leaves, right.leaves);
    let columns = left
        .columns
        .into_iter()
        .zip(right.columns)
        .map(|(mut from, also)| {
            add_new(&mut from, also);
            from
        })
        .collect();
    Underlying { leaves, columns }
}

/// [ISO/IEC 9075:1992 §7.10 SR9a iii, iv] What the columns of a set
/// operation `s` with CORRESPONDING are drawn from, of its operands'
/// `left` and `right`: as `( SELECT SL FROM TN1 ) OP ( SELECT SL FROM
/// TN2 )`, each column of SL that of the column of its name in each.
fn corresponded(cx: Context<'_>, s: &SetOperation, (left, right): (Underlying, Underlying)) -> Result<Underlying> {
    let names_of = |q: &QueryExpression| -> Result<Vec<Option<String>>> {
        Ok(bind_query_expression(cx, q, &Nesting::Query)?.columns.into_iter().map(|c| c.name).collect())
    };
    let (in_left, in_right) = (names_of(&s.left)?, names_of(&s.right)?);
    let sl: Vec<String> = match &s.corresponding {
        Correspondence::By(list) => list.clone(),
        Correspondence::Names | Correspondence::Position => {
            in_left.iter().flatten().filter(|n| in_right.contains(&Some((*n).clone()))).cloned().collect()
        }
    };
    let pick = |names: &[Option<String>], of: &Underlying| -> Underlying {
        let columns = sl
            .iter()
            .map(|n| names.iter().position(|c| c.as_ref() == Some(n)).and_then(|at| of.columns.get(at)).cloned().unwrap_or_default())
            .collect();
        Underlying { leaves: of.leaves.clone(), columns }
    };
    Ok(merged(pick(&in_left, &left), pick(&in_right, &right)))
}

/// What the columns of the `<query specification>` `spec`, the query
/// `query`, are drawn from [ISO/IEC 9075:1992 §7.9 SR5].
fn of_spec(cx: Context<'_>, query: &QueryExpression, spec: &QuerySpecification) -> Result<Underlying> {
    let mut leaves = Vec::new();
    spec.table_expression.from.iter().for_each(|r| reference_leaves(cx, r, &mut leaves));
    let (uses, selected) = (Uses::default(), Selected::default());
    bind_query_expression(Context { uses: Some(&uses), selected: Some(&selected), ..cx }, query, &Nesting::Query)?;
    let te = table_expression_uses(cx, spec)?;
    // Each column's own: a column of none would have no REFERENCES.
    let columns = selected
        .into_inner()
        .into_iter()
        .map(|mine| {
            let mut from = Vec::new();
            add_new(&mut from, mine.into_iter().chain(te.iter().cloned()).filter(|(t, _)| leaves.contains(t)).collect());
            from
        })
        .collect();
    Ok(Underlying { leaves, columns })
}

/// [ISO/IEC 9075:1992 §7.3 SR2] The columns the `<table expression>` of
/// `spec` references: its own, bound with `cx` as `SELECT 1` of it.
fn table_expression_uses(cx: Context<'_>, spec: &QuerySpecification) -> Result<Vec<(QualifiedName, String)>> {
    let one = ValueExpression::Value(ValueSpecification::Literal(Literal::Exact(Decimal::new(1_i128, 0_u32))));
    let select_list = SelectList::Values(vec![SelectSublist::Derived(SelectItem { value: one, alias: None, at: 0 })]);
    let te = QuerySpecification { distinct: false, select_list, table_expression: spec.table_expression.clone() };
    let uses = Uses::default();
    bind_query_expression(Context { uses: Some(&uses), selected: None, ..cx }, &QueryExpression::Spec(Box::new(te)), &Nesting::Query)?;
    Ok(uses.into_inner())
}

/// [ISO/IEC 9075:1992 §4.9, §7.10 SR14] Adds to `leaves` the leaf
/// underlying tables of `<table reference>` `r`: the table or view it
/// names, or those of the operands of a join or of a derived table's
/// query.
fn reference_leaves(cx: Context<'_>, r: &TableReference, leaves: &mut Vec<QualifiedName>) {
    match r {
        TableReference::Table(t) => add_new(leaves, vec![qualify(&t.table, cx.schema)]),
        TableReference::Derived(d) => query_leaves(cx, &d.query.query, leaves),
        TableReference::Joined(j) => {
            reference_leaves(cx, &j.left, leaves);
            reference_leaves(cx, &j.right, leaves);
        }
    }
}

/// Adds to `leaves` the leaf underlying tables of query `q`.
fn query_leaves(cx: Context<'_>, q: &QueryExpression, leaves: &mut Vec<QualifiedName>) {
    match q {
        QueryExpression::Spec(spec) => spec.table_expression.from.iter().for_each(|r| reference_leaves(cx, r, leaves)),
        QueryExpression::Values(_) => {}
        QueryExpression::SetOp(s) => {
            query_leaves(cx, &s.left, leaves);
            query_leaves(cx, &s.right, leaves);
        }
    }
}

/// Adds to `to` those of `more` it has not.
fn add_new<T: PartialEq>(to: &mut Vec<T>, more: Vec<T>) {
    for m in more {
        if !to.contains(&m) {
            to.push(m);
        }
    }
}

/// [ISO/IEC 9075:1992 §7.5 SR6d, §6.4 SR9] Notes in `uses` the columns
/// common column `i` of `item`, of a NATURAL or USING join, is drawn
/// from, as `COALESCE ( TA.C, TB.C )`: the join columns of its name of the
/// tables and views the query names, through nested joins too. A derived
/// table's are what its query references, which its `<table expression>`
/// does.
pub(super) fn note_common(binder: &Binder<'_>, item: &FromItem, i: usize, uses: &Uses) {
    let name = item.desc.columns.get(i).map(|c| &c.name);
    for it in binder.items.iter().filter(|it| !it.table.auth.is_empty()) {
        for &k in it.common.iter().filter(|&&k| it.desc.columns.get(k).map(|c| &c.name) == name) {
            let column = views::column_name(binder.db, &it.table, k).or_else(|| it.desc.columns.get(k).map(|c| c.name.clone()));
            uses.borrow_mut().extend(column.map(|c| (it.table.clone(), c)));
        }
    }
}
