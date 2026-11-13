//! [ISO/IEC 9075-2:2023 §7.18] The `<search or cycle clause>` of a `<with
//! list element>`: its Syntax Rules, and the element replaced by the
//! expanded element SR8 defines it as (`search_cycle/expansion.rs`),
//! which is then bound as any recursive element is.
//!
//! An element with one must be expandable [§7.17 SR3k]: recursive and
//! referencing no other query of its list, its query a UNION whose first
//! operand does not reference it and whose second is a `<query
//! specification>` [SR3j xi]. That `<query specification>` references the
//! element in a `<table reference>` of its FROM clause [§7.18 SR4].
//!
//! The expansion's sequence column of DEPTH FIRST and its path column are
//! arrays that grow by a row each iteration. Of the anchor's ARRAY of one
//! row [§6.49 SR2], and taking that type [§7.17 SR17c], they could not
//! grow, so they are of the greatest maximum cardinality (IL008) instead,
//! and BREADTH FIRST's level is INTEGER, not the type of the literal 0
//! (RULES.md D70).

mod expansion;

use super::{OutColumn, rule};
use crate::ast::{
    Correspondence, CycleClause, Literal, NamedTable, QueryExpression, QuerySpecification, SetOperation, SetOperator, TableReference,
    ValueExpression, ValueSpecification, WithElement, WithQuery,
};
use crate::error::{At, Code, Result, SqlError};
use crate::value::DataType;
use crate::value::types::MAX_ARRAY_CARDINALITY;

/// A potentially recursive list with each element that has a `<search or
/// cycle clause>` expanded [§7.18 SR8], and of each element, by place, the
/// places of its columns that are arrays of the greatest maximum
/// cardinality (RULES.md D70).
pub(super) struct Expanded {
    /// The list.
    pub(super) list: WithQuery,
    /// The places of each element's widened columns.
    pub(super) widened: Vec<Vec<usize>>,
}

/// [ISO/IEC 9075-2:2023 §7.18 SR8] `w` with each element that has a
/// `<search or cycle clause>` expanded, its query name dependency graph
/// `arcs` [§7.17 SR3j ii]; none if no element has one.
///
/// # Errors
///
/// SQLCODE -103 if an element with one is not expandable [§7.17 SR3k] or
/// breaks a Syntax Rule of §7.18; -900 for valid SQL not supported.
pub(super) fn expanded(w: &WithQuery, arcs: &[Vec<usize>]) -> Result<Option<Expanded>> {
    if !w.elements.iter().any(has_clause) {
        return Ok(None);
    }
    let mut list = w.clone();
    let mut widened = Vec::new();
    for (place, e) in list.elements.iter_mut().enumerate() {
        if !has_clause(e) {
            widened.push(Vec::new());
            continue;
        }
        let parts = parts(e, arcs.get(place).map_or(&[], Vec::as_slice), place)?;
        let (element, wide) = expansion::expanded(&parts)?;
        *e = element;
        widened.push(wide);
    }
    Ok(Some(Expanded { list, widened }))
}

/// [ISO/IEC 9075-2:2023 §7.17 SR3k] Refuses the element `e`, of a list
/// bound without recursion, if it has a `<search or cycle clause>`: it is
/// not recursive, so not expandable.
///
/// # Errors
///
/// SQLCODE -103 if it has one.
pub(super) fn refuse_clause(e: &WithElement) -> Result<()> {
    if has_clause(e) {
        return Err(not_expandable(e, "it is not recursive"));
    }
    Ok(())
}

/// [RULES.md D70] `columns` with each at `places` that is an array of the
/// greatest maximum cardinality.
pub(super) fn widened(mut columns: Vec<OutColumn>, places: &[usize]) -> Vec<OutColumn> {
    for (p, c) in columns.iter_mut().enumerate() {
        if let (true, DataType::Array(a)) = (places.contains(&p), &c.data_type) {
            c.data_type = DataType::array(a.element.clone(), MAX_ARRAY_CARDINALITY);
        }
    }
    columns
}

/// Whether `e` has a `<search or cycle clause>`.
fn has_clause(e: &WithElement) -> bool {
    e.search.is_some() || e.cycle.is_some()
}

/// What SR8 is applied to: the element, its parts SR3 and SR4 name, and
/// its mark values.
pub(super) struct Parts<'e> {
    /// The element, WLEC.
    pub(super) element: &'e WithElement,
    /// Its `<with column list>`, WCL.
    pub(super) columns: &'e [String],
    /// Its UNION, OP, of the operands TLO and TRO.
    pub(super) union: &'e SetOperation,
    /// The second operand, TRO.
    pub(super) step: &'e QuerySpecification,
    /// The `<table reference>` of TRO's FROM clause that references the
    /// element, WQNTR.
    pub(super) reference: &'e NamedTable,
    /// The `<cycle mark value>` and `<non-cycle mark value>`, CMV and CMD,
    /// of a `<cycle clause>`.
    pub(super) marks: Option<(ValueExpression, ValueExpression)>,
}

impl Parts<'_> {
    /// WQNCRN: the `<correlation name>` of WQNTR, or the query name
    /// \[SR4\].
    pub(super) fn correlation(&self) -> &str {
        self.reference.correlation.as_deref().unwrap_or(&self.element.name)
    }
}

/// [ISO/IEC 9075-2:2023 §7.17 SR3j xi, SR3k; §7.18 SR3 to SR7] The parts
/// of the element `e` at `place`, which references the elements at `on`.
fn parts<'e>(e: &'e WithElement, on: &[usize], place: usize) -> Result<Parts<'e>> {
    let (union, step) = expandable(e, on, place)?;
    let Some(columns) = e.columns.as_deref() else {
        let msg = format!("[ISO/IEC 9075-2:2023 §7.17 SR3j i] query {} of a WITH RECURSIVE list has no <with column list>", e.name);
        return Err(rule(msg).or_at(e.at));
    };
    named(e, columns)?;
    let Some(reference) = referenced(&step.table_expression.from, &e.name) else {
        let msg = format!(
            "[ISO/IEC 9075-2:2023 §7.18 SR4] the second operand of query {0}'s UNION references {0} only within a derived table, not in its FROM clause",
            e.name
        );
        return Err(rule(msg).or_at(e.at));
    };
    let marks = e.cycle.as_ref().map(marks).transpose()?;
    Ok(Parts { element: e, columns, union, step, reference, marks })
}

/// [ISO/IEC 9075-2:2023 §7.17 SR3j xi, SR3k] The UNION and the second
/// operand of the element `e` at `place`, which references the elements at
/// `on`, if it is expandable.
fn expandable<'e>(e: &'e WithElement, on: &[usize], place: usize) -> Result<(&'e SetOperation, &'e QuerySpecification)> {
    if !on.contains(&place) {
        return Err(not_expandable(e, "it is not recursive"));
    }
    if on.len() > 1 {
        return Err(not_expandable(e, "it references another query of its list [SR3j xi 3]"));
    }
    let QueryExpression::SetOp(union) = &e.query else { return Err(not_expandable(e, "its query is not a UNION [SR3j xi 2]")) };
    if union.op != SetOperator::Union {
        return Err(not_expandable(e, "its query is not a UNION [SR3j xi 2]"));
    }
    if references(&union.left, &e.name) {
        return Err(not_expandable(e, "the first operand of its UNION references it [SR3j xi 2]"));
    }
    let QueryExpression::Spec(step) = &union.right else {
        return Err(not_expandable(e, "the second operand of its UNION is not a <query specification> [SR3j xi 2]"));
    };
    if !matches!(union.corresponding, Correspondence::Position) {
        let msg = format!("a <search or cycle clause> of a UNION CORRESPONDING, as query {}'s is, is not supported", e.name);
        return Err(SqlError::new(Code::Unsupported, msg).or_at(e.at));
    }
    Ok((union, step))
}

/// [ISO/IEC 9075-2:2023 §7.17 SR3k] The refusal of the `<search or cycle
/// clause>` of `e`, not expandable as `why` says.
fn not_expandable(e: &WithElement, why: &str) -> SqlError {
    let msg = format!("[ISO/IEC 9075-2:2023 §7.17 SR3k] query {} has a <search or cycle clause>, but {why}", e.name);
    rule(msg).or_at(e.at)
}

/// Whether `q` references the query name `name`, not one of a `<with
/// clause>` within it [ISO/IEC 9075-2:2023 §7.17 SR3i].
fn references(q: &QueryExpression, name: &str) -> bool {
    let mut found = false;
    q.for_each_named(&mut |t, query| found |= !query && t.table.auth.is_none() && t.table.ident == name);
    found
}

/// [ISO/IEC 9075-2:2023 §7.18 SR4] The `<table reference>` simply
/// contained in `from` that is the query name `name`, if one is: in the
/// FROM clause or a joined table there, not in a derived table.
pub(super) fn referenced<'f>(from: &'f [TableReference], name: &str) -> Option<&'f NamedTable> {
    let mut todo: Vec<&TableReference> = from.iter().rev().collect();
    while let Some(r) = todo.pop() {
        match r {
            TableReference::Table(t) => {
                if t.table.auth.is_none() && t.table.ident == name {
                    return Some(t);
                }
            }
            TableReference::Joined(j) => todo.extend([&j.right, &j.left]),
            TableReference::Derived(_) | TableReference::Unnest(_) => {}
        }
    }
    None
}

/// [ISO/IEC 9075-2:2023 §7.18 SR5a, SR5b, SR6a, SR6b, SR7c i] That the
/// columns the clauses of `e` name, of its `<with column list>`
/// `columns`, are named as the rules say.
fn named(e: &WithElement, columns: &[String]) -> Result<()> {
    if let Some(s) = &e.search {
        if columns.contains(&s.sequence) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §7.18 SR5a] the sequence column {} is a column of query {}",
                s.sequence, e.name
            ))
            .or_at(s.at));
        }
        listed(&s.columns, columns, "§7.18 SR5b").or_at(s.at)?;
    }
    if let Some(c) = &e.cycle {
        listed(&c.columns, columns, "§7.18 SR6a").or_at(c.at)?;
        if new_names(c, columns).broken() {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §7.18 SR6b] the cycle mark column {} and the path column {} are two names of no column of query {}",
                c.mark, c.path, e.name
            );
            return Err(rule(msg).or_at(c.at));
        }
    }
    if let (Some(s), Some(c)) = (&e.search, &e.cycle)
        && (s.sequence == c.mark || s.sequence == c.path)
    {
        let msg =
            format!("[ISO/IEC 9075-2:2023 §7.18 SR7c i] the sequence column {} is the cycle mark column or the path column", s.sequence);
        return Err(rule(msg).or_at(c.at));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §7.18 SR6b] Whether the cycle mark column and the
/// path column of `c` are two names, neither of `columns`.
fn new_names(c: &CycleClause, columns: &[String]) -> NewNames {
    NewNames { apart: c.mark != c.path, mark_new: !columns.contains(&c.mark), path_new: !columns.contains(&c.path) }
}

/// [ISO/IEC 9075-2:2023 §7.18 SR6b] The conditions on a `<cycle clause>`'s
/// two new columns.
struct NewNames {
    /// The cycle mark column and the path column are not one name.
    apart: bool,
    /// The cycle mark column is no column of the `<with column list>`.
    mark_new: bool,
    /// The path column is no column of the `<with column list>`.
    path_new: bool,
}

impl NewNames {
    /// Whether some condition does not hold.
    fn broken(&self) -> bool {
        !(self.apart && self.mark_new && self.path_new)
    }
}

/// [ISO/IEC 9075-2:2023 §7.18 SR5b, SR6a] That each of `names` is one of
/// `columns`, none named twice, as `clause` says.
fn listed(names: &[String], columns: &[String], clause: &str) -> Result<()> {
    for (n, name) in names.iter().enumerate() {
        if !columns.contains(name) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 {clause}] {name} is no column of the <with column list>")));
        }
        if names.iter().take(n).any(|earlier| earlier == name) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 {clause}] column {name} is named twice")));
        }
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §7.18 SR1, SR6c] The `<cycle mark value>` and
/// `<non-cycle mark value>` of `c`: TRUE and FALSE if it writes none; else
/// two literals of one type, each a character string of length 1 or a
/// Boolean, that are not equal.
fn marks(c: &CycleClause) -> Result<(ValueExpression, ValueExpression)> {
    let Some((to, otherwise)) = &c.marks else {
        return Ok((boolean(true), boolean(false)));
    };
    if let (Some(a), Some(b)) = (literal(to), literal(otherwise))
        && distinct_marks(a, b)
    {
        return Ok((to.clone(), otherwise.clone()));
    }
    let msg = "[ISO/IEC 9075-2:2023 §7.18 SR6c] the cycle mark values are two unequal literals, both character strings of length 1 or both Boolean, TRUE or FALSE";
    Err(rule(msg).or_at(c.at))
}

/// The `<literal>` `v` is, in parentheses or not, if it is one.
fn literal(mut v: &ValueExpression) -> Option<&Literal> {
    while let ValueExpression::Nested(inner) = v {
        v = inner;
    }
    if let ValueExpression::Value(ValueSpecification::Literal(l)) = v { Some(l) } else { None }
}

/// [ISO/IEC 9075-2:2023 §7.18 SR6c] Whether `a` and `b` are mark values of
/// one type that are not equal: TRUE and FALSE, or two different character
/// strings of length 1. UNKNOWN is neither equal to the other nor not.
fn distinct_marks(a: &Literal, b: &Literal) -> bool {
    if let (Literal::Boolean(Some(x)), Literal::Boolean(Some(y))) = (a, b) {
        return x != y;
    }
    if let (Literal::Character(x), Literal::Character(y)) = (a, b) {
        return x.chars().count() == 1 && y.chars().count() == 1 && x != y;
    }
    false
}

/// The `<boolean literal>` of `value`.
fn boolean(value: bool) -> ValueExpression {
    ValueExpression::Value(ValueSpecification::Literal(Literal::Boolean(Some(value))))
}
