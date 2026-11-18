//! The query expressions a `<query expression>` contains, and the parts
//! of a `<query specification>` the rules of a potentially recursive
//! `<with list>` look in [ISO/IEC 9075-2:2023 §7.17 SR3f, SR3h].

use crate::ast::{
    JoinSpecification, QueryExpression, QuerySpecification, RowElement, SearchCondition, SelectItem, SelectList, SelectSublist,
    TableReference, ValueExpression, WindowOver,
};

/// [ISO/IEC 9075-2:2023 §7.17 SR3h] Whether `q` contains a potentially
/// recursive `<with list>`.
pub(in crate::binder) fn has_recursive_list(q: &QueryExpression) -> bool {
    any_query(q, &|x| matches!(x, QueryExpression::With(w) if w.recursive))
}

/// Whether `pred` holds of `q` or of a query expression it contains: an
/// operand, an element's query or body, a derived table's, a subquery's.
pub(super) fn any_query(q: &QueryExpression, pred: &dyn Fn(&QueryExpression) -> bool) -> bool {
    if pred(q) {
        return true;
    }
    let mut nested: Vec<&QueryExpression> = Vec::new();
    match q {
        QueryExpression::SetOp(s) => nested.extend([&s.left, &s.right]),
        QueryExpression::With(w) => nested.extend(w.elements.iter().map(|e| &e.query).chain([&w.body])),
        QueryExpression::Ordered(o) => nested.push(&o.body),
        QueryExpression::Values(rows) => row_values(rows).into_iter().for_each(|v| v.for_each_subquery(&mut |s| nested.push(&s.query))),
        QueryExpression::Spec(spec) => {
            for v in selected(spec) {
                v.for_each_subquery(&mut |s| nested.push(&s.query));
            }
            for c in conditions(spec) {
                c.for_each_subquery(&mut |s| nested.push(&s.query));
            }
            let mut refs: Vec<&TableReference> = spec.table_expression.from.iter().collect();
            while let Some(r) = refs.pop() {
                match r {
                    TableReference::Table(_) | TableReference::Hybrid(_) => {}
                    TableReference::Derived(d) => nested.push(&d.query.query),
                    TableReference::Unnest(u) => {
                        for v in &u.collections {
                            v.for_each_subquery(&mut |s| nested.push(&s.query));
                        }
                    }
                    TableReference::Json(j) => {
                        for v in j.operands() {
                            v.for_each_subquery(&mut |s| nested.push(&s.query));
                        }
                    }
                    TableReference::Joined(j) => refs.extend([&j.left, &j.right]),
                    TableReference::Recognized(r) => {
                        refs.push(&r.input);
                        r.clause.for_each_subquery(&mut |s| nested.push(&s.query));
                    }
                    TableReference::Polymorphic(p) => {
                        nested.extend(p.invocation.subqueries(&mut |_| {}).into_iter().map(|s| &s.query));
                    }
                }
            }
        }
    }
    nested.into_iter().any(|n| any_query(n, pred))
}

/// [ISO/IEC 9075-2:2023 §7.17 SR3f] Whether `q` is a query specification
/// that simply contains row pattern recognition: of a table primary, or
/// of a window of its window clause or its select list.
pub(super) fn recognizes(q: &QueryExpression) -> bool {
    let QueryExpression::Spec(spec) = q else { return false };
    let te = &spec.table_expression;
    if te.window.iter().any(|d| d.specification.recognition.is_some()) {
        return true;
    }
    let inline = |v: &ValueExpression| {
        let ValueExpression::Window(w) = v else { return false };
        let WindowOver::Specification(s) = &w.over else { return false };
        s.recognition.is_some()
    };
    if selected(spec).into_iter().any(|v| v.any(&inline)) {
        return true;
    }
    let mut refs: Vec<&TableReference> = te.from.iter().collect();
    while let Some(r) = refs.pop() {
        match r {
            TableReference::Recognized(_) => return true,
            TableReference::Joined(j) => refs.extend([&j.left, &j.right]),
            TableReference::Table(_)
            | TableReference::Derived(_)
            | TableReference::Unnest(_)
            | TableReference::Json(_)
            | TableReference::Polymorphic(_)
            | TableReference::Hybrid(_) => {}
        }
    }
    false
}

/// The value expressions of the select list of `spec`.
pub(super) fn selected(spec: &QuerySpecification) -> Vec<&ValueExpression> {
    match &spec.select_list {
        SelectList::Star => Vec::new(),
        SelectList::Values(items) => items
            .iter()
            .filter_map(|i| match i {
                SelectSublist::Derived(SelectItem { value, .. }) | SelectSublist::Fields { value, .. } => Some(value),
                SelectSublist::All(_) => None,
            })
            .collect(),
    }
}

/// The search conditions of `spec`'s own: its WHERE and HAVING clauses',
/// and its join conditions'.
pub(super) fn conditions(spec: &QuerySpecification) -> Vec<&SearchCondition> {
    let te = &spec.table_expression;
    let mut out: Vec<&SearchCondition> = te.where_clause.iter().chain(&te.having).collect();
    let mut refs: Vec<&TableReference> = te.from.iter().collect();
    while let Some(r) = refs.pop() {
        if let TableReference::Joined(j) = r {
            if let Some(JoinSpecification::On(c)) = &j.on {
                out.push(c);
            }
            refs.extend([&j.left, &j.right]);
        }
    }
    out
}

/// The value expressions of the rows of a `<table value constructor>`.
pub(super) fn row_values(rows: &[crate::ast::ValuesRow]) -> Vec<&ValueExpression> {
    rows.iter()
        .flat_map(|r| &r.elements)
        .filter_map(|e| match e {
            RowElement::Value(v, _) => Some(v),
            RowElement::Null(_) | RowElement::Default(_) => None,
        })
        .collect()
}
