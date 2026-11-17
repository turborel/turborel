//! Where an INSERT whose query is of another degree than its columns is
//! in error [X3.135-1986 §8.7 SR5].

use crate::ast::{QueryExpression, RowElement, SelectList, SelectSublist};

/// Where an INSERT's `query` of another degree than its `m` columns is
/// placed: at its first value or select list item too many, or, of too
/// few, at its first row or item.
pub(super) fn degree_at(query: &QueryExpression, m: usize) -> Option<usize> {
    match query.body() {
        QueryExpression::Values(rows) => {
            let row = rows.first()?;
            Some(row.elements.get(m).map_or(row.at, |e| match e {
                RowElement::Value(_, at) | RowElement::Null(at) | RowElement::Default(at) => *at,
            }))
        }
        QueryExpression::Spec(spec) => {
            let SelectList::Values(items) = &spec.select_list else { return None };
            let at = |i: &SelectSublist| if let SelectSublist::Derived(d) = i { Some(d.at) } else { None };
            items.get(m).or(items.first()).and_then(at)
        }
        QueryExpression::SetOp(_) | QueryExpression::With(_) | QueryExpression::Ordered(_) => None,
    }
}
