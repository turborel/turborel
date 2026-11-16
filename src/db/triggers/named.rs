//! [ISO/IEC 9075-2:2023 §11.31 SR7g, §11.33 SR4g] The tables and views a
//! triggered action names, as written: those it references, which are
//! not dropped while it is defined unless with CASCADE. Its transition
//! table names name none, nor does a transition variable: of its subject
//! table, the action names it only by a `<table name>`.

use std::slice;

use crate::ast::{
    InsertSource, MatchedAction, MergeStatement, MergeWhen, QueryExpression, RowElement, SearchCondition, SetClause, SqlArgument,
    SqlStatement, Subquery, TableName, UpdateSource, ValueExpression, ValuesRow, from_table_names,
};
use crate::binder::qualify;
use crate::catalog::{QualifiedName, TransitionNames};

/// The tables and views the statements `action` and the condition `when`
/// name, once each, those written without a schema in `schema`; not a
/// transition table name of `names`.
pub(in crate::db) fn named(
    action: &[SqlStatement],
    when: Option<&SearchCondition>,
    (schema, names): (&str, &TransitionNames),
) -> Vec<QualifiedName> {
    let mut written: Vec<TableName> = Vec::new();
    if let Some(c) = when {
        condition(c, &mut written);
    }
    for stmt in action {
        statement(stmt, &mut written);
    }
    let transition = |t: &TableName| t.auth.is_none() && [&names.old_table, &names.new_table].into_iter().flatten().any(|n| *n == t.ident);
    let mut out: Vec<QualifiedName> = Vec::new();
    for t in written.iter().filter(|t| !transition(t)) {
        let qn = qualify(t, schema);
        if !out.contains(&qn) {
            out.push(qn);
        }
    }
    out
}

/// Adds to `out` the table names written in `stmt`.
fn statement(stmt: &SqlStatement, out: &mut Vec<TableName>) {
    match stmt {
        SqlStatement::Insert(ins) => {
            out.push(ins.table.clone());
            if let InsertSource::Query(q) = &ins.source {
                query(q, out);
            }
        }
        SqlStatement::Select { query: spec, .. } => query(&QueryExpression::Spec(spec.clone()), out),
        SqlStatement::UpdateSearched { table, set, where_clause } => {
            out.push(table.clone());
            set_clauses(set, out);
            if_condition(where_clause.as_ref(), out);
        }
        SqlStatement::DeleteSearched { table, where_clause } => {
            out.push(table.clone());
            if_condition(where_clause.as_ref(), out);
        }
        SqlStatement::Merge(m) => merge(m, out),
        SqlStatement::Call(call) => values(call.arguments.iter().filter_map(argued), out),
        // [ISO/IEC 9075-2:2023 §16.2] A function's `<return value>`.
        SqlStatement::Return(r) => values(r.value.iter(), out),
        // What a triggered action may not contain names nothing it reads.
        SqlStatement::GetDiagnostics(_)
        | SqlStatement::Close(_)
        | SqlStatement::SetCollation(_)
        | SqlStatement::Commit
        | SqlStatement::DeletePositioned { .. }
        | SqlStatement::Truncate { .. }
        | SqlStatement::AlterTable { .. }
        | SqlStatement::Drop { .. }
        | SqlStatement::Schema { .. }
        | SqlStatement::Fetch { .. }
        | SqlStatement::Dynamic(_)
        | SqlStatement::Execute(_)
        | SqlStatement::DeclareTable(_)
        | SqlStatement::Open(_)
        | SqlStatement::Rollback
        | SqlStatement::SetTransaction(_)
        | SqlStatement::SetCatalog(_)
        | SqlStatement::SetSchema(_)
        | SqlStatement::SetTimeZone(_)
        | SqlStatement::SetSessionAuthorization(_)
        | SqlStatement::Connection(_)
        | SqlStatement::SetSetting { .. }
        | SqlStatement::SetDatabase { .. }
        | SqlStatement::Index(_)
        | SqlStatement::User(_)
        | SqlStatement::Served(_)
        | SqlStatement::SetConstraints { .. }
        | SqlStatement::UpdatePositioned { .. } => {}
    }
}

/// The value expression of a CALL's argument `a`, if it is one.
fn argued(a: &SqlArgument) -> Option<&ValueExpression> {
    if let SqlArgument::Value(v) = a { Some(v) } else { None }
}

/// Adds to `out` the table names written in the subqueries of `of`: a
/// CALL's arguments, or a RETURN's value.
fn values<'v>(of: impl Iterator<Item = &'v ValueExpression>, out: &mut Vec<TableName>) {
    for v in of {
        v.for_each_subquery(&mut |s| subquery(s, out));
    }
}

/// Adds to `out` the table names written in MERGE `m`: its target's,
/// its source's and those of its subqueries.
fn merge(m: &MergeStatement, out: &mut Vec<TableName>) {
    out.push(m.target.clone());
    let mut tables = Vec::new();
    from_table_names(slice::from_ref(&m.source), &mut tables);
    out.extend(tables.into_iter().cloned());
    condition(&m.on, out);
    for clause in &m.clauses {
        match clause {
            MergeWhen::Matched { condition: c, action, .. } => {
                if_condition(c.as_ref(), out);
                if let MatchedAction::Update(set) = action {
                    set_clauses(set, out);
                }
            }
            MergeWhen::NotMatched { condition: c, insert, .. } => {
                if_condition(c.as_ref(), out);
                row(&insert.values, out);
            }
        }
    }
}

/// Adds to `out` the table names written in `q` and its subqueries.
fn query(q: &QueryExpression, out: &mut Vec<TableName>) {
    let mut tables = Vec::new();
    q.table_names(&mut tables);
    out.extend(tables.into_iter().cloned());
}

/// Adds to `out` the table names written in `c`, if there is one.
fn if_condition(c: Option<&SearchCondition>, out: &mut Vec<TableName>) {
    if let Some(c) = c {
        condition(c, out);
    }
}

/// Adds to `out` the table names written in the subqueries of `c`.
fn condition(c: &SearchCondition, out: &mut Vec<TableName>) {
    c.for_each_subquery(&mut |s| subquery(s, out));
}

/// Adds to `out` the table names written in the subquery `s`.
fn subquery(s: &Subquery, out: &mut Vec<TableName>) {
    query(&s.query, out);
}

/// Adds to `out` the table names written in the subqueries of the
/// values of `set`.
fn set_clauses(set: &[SetClause], out: &mut Vec<TableName>) {
    for s in set {
        let values = s.element.iter().chain(if let UpdateSource::Value(v) = &s.value { Some(v) } else { None });
        values.for_each(|v| v.for_each_subquery(&mut |q| subquery(q, out)));
    }
}

/// Adds to `out` the table names written in the subqueries of the
/// values of `r`.
fn row(r: &ValuesRow, out: &mut Vec<TableName>) {
    for e in &r.elements {
        if let RowElement::Value(v, _) = e {
            v.for_each_subquery(&mut |s| subquery(s, out));
        }
    }
}
