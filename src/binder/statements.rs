//! Binding of the statements that use a single table: the updatability of a
//! cursor [X3.135-1986 §8.3 SR4b, §5.25 SR11], single-row SELECT [X3.135-1986
//! §8.10], and searched and positioned UPDATE and DELETE [X3.135-1986 §8.4,
//! §8.5, §8.11, §8.12], with the assignment rules they share.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::views::{Source, ViewCheck, write_target};
use super::{Binder, Bound, Context, FromItem, Scope, bind_query_specification, no_column, qualify, require, rule};
use crate::ast::{
    CursorSpecification, QuerySpecification, SearchCondition, SetClause, TableExpression, TableName, TableReference, UpdateSource,
};
use crate::catalog::Actions;
use crate::catalog::{Catalog, ColumnDesc, QualifiedName, TableDesc};
use crate::error::{At, Code, Result, SqlError};
use crate::logical::{Expr, LogicalPlan, and_all};
use crate::params::ParameterType;
use crate::value::precedence::in_precedence_list;
use crate::value::{DataType, Value};

/// [X3.135-1986 §8.3 SR4b; ISO/IEC 9075:1992 §13.1 SR3] the table of a
/// cursor is updatable when it is no set operation and its `<query
/// specification>` is updatable (`updatable_query`). Returns that `<query
/// specification>`. Whether the cursor is, with ORDER BY, SCROLL or its
/// `<updatability clause>`, is §13.1 SR5's (`db/cursor.rs`).
pub fn updatable(c: &CursorSpecification) -> Option<&QuerySpecification> {
    // [ISO/IEC 9075:1992 §7.10 SR4] no set operation.
    let s = c.query.spec()?;
    updatable_query(s).then_some(s)
}

/// [X3.135-1986 §5.25 SR11; ISO/IEC 9075:1992 §7.9 SR12] Whether a `<query
/// specification>` is updatable as written: (a) no DISTINCT, (b) a select
/// list of `<column specification>`s only, (c) exactly one `<table
/// reference>`, (e) no GROUP BY or HAVING. That its table is updatable (c),
/// and (d) that no subquery of its WHERE clause reads the table it
/// changes, need the catalog and the bound query (`binder/views.rs`,
/// `db/cursor.rs`): SQL-86's rule of no subquery at all [X3.135-1986 §5.25
/// SR11d] is SQL-92's (d).
pub(super) fn updatable_query(s: &QuerySpecification) -> bool {
    // [X3.135-1986 §5.25 SR4] "*" is a list of <column specification>s.
    !s.distinct && s.select_list.columns_only() && single_ungrouped_table(&s.table_expression)
}

/// [X3.135-1986 §8.3 SR4b, §5.25 SR11c] Whether the cursor `c` is updatable:
/// as `updatable` says, and the table its `<from clause>` names is an
/// updatable table, a base table or an updatable view [X3.135-1986 §6.5
/// SR3], an unqualified name being of `schema`. Returns that table's name.
pub fn updatable_in(c: &CursorSpecification, catalog: &dyn Catalog, schema: &str) -> Option<QualifiedName> {
    let s = updatable(c)?;
    let name = qualify(&s.table_expression.from.first()?.named()?.table, schema);
    (catalog.table(&name).is_some() || catalog.view(&name).is_some_and(|v| v.updates.is_some())).then_some(name)
}

/// [X3.135-1986 §5.25 SR11c, SR11e] one `<table reference>`, naming a
/// table [ISO/IEC 9075:1992 §7.5 SR10, §7.9 SR12c], and no GROUP BY or
/// HAVING.
fn single_ungrouped_table(te: &TableExpression) -> bool {
    matches!(te.from.as_slice(), [TableReference::Table(t)] if t.columns.is_none()) && te.group_by.is_none() && te.having.is_none()
}

/// [X3.135-1986 §8.10] the `<query specification>` of a `<select statement>`.
/// SQL-86's rule that it have no GROUP BY or HAVING, and identify no
/// grouped view [X3.135-1986 §8.10 SR2], is Entry SQL's alone [ISO/IEC
/// 9075:1992 §13.5, Leveling Rules]: that it gives one row at most is
/// checked as it runs.
///
/// # Errors
///
/// It breaks a rule of its query.
pub fn bind_select(cx: Context<'_>, query: &QuerySpecification) -> Result<Bound> {
    bind_query_specification(cx, query, &[])
}

/// The rule to cite when a value is assigned to a column: the clause that
/// applies store assignment [ISO/IEC 9075:1992 §9.2] to it.
#[derive(Clone, Copy)]
pub struct AssignRule {
    /// The clause and Syntax Rule, e.g. "§13.8 SR6".
    pub rule: &'static str,
}

/// The assignment rule of INSERT.
pub const INSERT_RULE: AssignRule = AssignRule { rule: "§13.8 SR6" };
/// The assignment rule of a searched UPDATE.
pub const UPDATE_SEARCHED_RULE: AssignRule = AssignRule { rule: "§13.10 SR5" };
/// The assignment rule of a positioned UPDATE.
pub const UPDATE_POSITIONED_RULE: AssignRule = AssignRule { rule: "§13.9 SR10" };

/// [ISO/IEC 9075:1992 §9.2 SR2] Can a value of type `t` be stored in
/// `col`? As `assignable` says. SQL-86's rules that a string be no longer
/// than the column and only an exact number go to an exact column
/// [X3.135-1986 §8.7 SR6, §8.11 SR8, §8.12 SR6] are Entry SQL's [ISO/IEC
/// 9075:1992 §13.8, §13.9, §13.10, Leveling Rules]: a longer string is
/// checked as it is stored [§9.2 GR3b].
///
/// # Errors
///
/// SQLCODE -103 citing `r` if it cannot.
pub fn check_assignment(col: &ColumnDesc, t: &DataType, r: AssignRule) -> Result<()> {
    if assignable(&col.data_type, t) {
        return Ok(());
    }
    Err(rule(format!("[ISO/IEC 9075:1992 {}, §9.2 SR2] cannot assign a {t} value to column {} ({})", r.rule, col.name, col.data_type)))
}

/// [ISO/IEC 9075:1992 §9.2 SR2] Can a value of type `t` be stored in a
/// target of type `target`? A character string in a character string, a
/// binary string in a binary string [ISO/IEC 9075-2:2023 §9.2 SR3], a
/// number in a number, a datetime in a
/// datetime of its fields, an interval in a comparable interval.
/// \[EXTENSION\] Where either is a type beyond the standard, which SR2
/// does not speak of (RULES.md D53): a value comparable with the target,
/// as its kind says, or a character string, if the target's kind takes
/// one (`SqlType::assigns_text`). [ISO/IEC 9075-2:2023 §9.2 SR2d, SR2e]
/// An array in an array whose element type its elements are assignable
/// to, a row in a row of its degree, field by field. [§4.9.5, §4.13, §9.2
/// SR3] A distinct type's value in a site of it, or of its source type
/// alone; in a site of it, a value of it, or one its function from its
/// source takes, implicitly invocable (`distinct_assignable`).
pub(crate) fn assignable(target: &DataType, t: &DataType) -> bool {
    if let Some(fits) = distinct_assignable(target, t) {
        return fits;
    }
    if let (DataType::Array(a), DataType::Array(b)) = (target, t) {
        return assignable(&a.element, &b.element);
    }
    if let (DataType::Row(a), DataType::Row(b)) = (target, t) {
        return a.fields.len() == b.fields.len() && a.fields.iter().zip(&b.fields).all(|(x, y)| assignable(&x.data_type, &y.data_type));
    }
    let composite = |either: &DataType| matches!(either, DataType::Array(_) | DataType::Row(_));
    if composite(target) || composite(t) {
        return false;
    }
    if let DataType::Extension(e) = target {
        return target.comparable(t) || (t.is_character() && e.kind.assigns_text());
    }
    if let DataType::Extension(_) = t {
        return target.comparable(t);
    }
    let temporal = target.is_datetime() || t.is_datetime() || target.is_interval() || t.is_interval();
    let datetime = if temporal { target.comparable(t) } else { true };
    target.is_character() == t.is_character() && target.is_binary() == t.is_binary() && datetime
}

/// [ISO/IEC 9075-2:2023 §4.9.5, §4.13, §9.2 SR3] Whether a value of `t` is
/// assigned to a site of `target`, if either is a distinct type: one of
/// the distinct type alone, as it has no subtype; or by an appropriate
/// user-defined cast function. Its function from its source is one if the
/// source's designator is in `t`'s type precedence list [§9.7] and `t` is
/// assignable to the source, its parameter [§9.18]; its function to its
/// source is one if `target` is its source, the function's result type.
/// None if neither is a distinct type.
pub(crate) fn distinct_assignable(target: &DataType, t: &DataType) -> Option<bool> {
    if let (DataType::Distinct(_), DataType::Distinct(_)) = (target, t) {
        return Some(target == t);
    }
    // Of every source type a distinct type may have, a type whose list
    // holds the source's designator is assignable to it, the second
    // condition kept as the rule states it.
    if let DataType::Distinct(d) = target {
        return Some(in_precedence_list(&d.source, t) && assignable(&d.source, t));
    }
    if let DataType::Distinct(d) = t {
        return Some(*target == d.source);
    }
    None
}

/// Which statement changes the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteKind {
    /// `<update statement: searched>` [X3.135-1986 §8.12].
    UpdateSearched,
    /// `<update statement: positioned>` [X3.135-1986 §8.11].
    UpdatePositioned,
    /// `<delete statement: searched>` [X3.135-1986 §8.5].
    DeleteSearched,
    /// `<delete statement: positioned>` [X3.135-1986 §8.4].
    DeletePositioned,
}

impl WriteKind {
    /// The rule a read-only table breaks [X3.135-1986 §4.4].
    fn read_only_rule(self) -> &'static str {
        match self {
            WriteKind::UpdateSearched => "§8.12 SR2",
            WriteKind::UpdatePositioned => "§8.11 SR3",
            WriteKind::DeleteSearched => "§8.5 SR2",
            WriteKind::DeletePositioned => "§8.4 SR3",
        }
    }

    fn clause(self) -> &'static str {
        match self {
            WriteKind::UpdateSearched => "§8.12",
            WriteKind::UpdatePositioned => "§8.11",
            WriteKind::DeleteSearched => "§8.5",
            WriteKind::DeletePositioned => "§8.4",
        }
    }
}

/// The object table of an UPDATE or DELETE, its SET clauses, and the plan
/// that finds its object rows.
#[derive(Debug, Clone)]
pub struct BoundWrite {
    /// The table or view the statement names.
    pub named: QualifiedName,
    /// The object table: it, or the base table of the updatable view
    /// [X3.135-1986 §6.5 GR2].
    pub table: QualifiedName,
    /// Its description.
    pub desc: TableDesc,
    /// Column position and new value (`None` is NULL), in the order written.
    pub set: Vec<(usize, Option<Expr>)>,
    /// Scan of the table, filtered by the views' conditions and the
    /// `<search condition>`, if any.
    pub rows: LogicalPlan,
    /// [ISO/IEC 9075:1992 §11.19 GR11] The views WITH CHECK OPTION it
    /// spans, which every updated row is checked against.
    pub checks: Vec<ViewCheck>,
    /// The authorization identifier it runs under: CURRENT_USER, which a
    /// default of its referential actions may be [ISO/IEC 9075:1992 §11.5
    /// GR1c].
    pub auth: String,
    /// [ISO/IEC 9075-2:2023 §15.14 GR9] Each generated column of the object
    /// table, by its position, its generation expression bound over the row.
    pub generated: Vec<(usize, Expr)>,
}

impl BoundWrite {
    /// Every base table the subqueries of its SET values read.
    pub fn set_tables(&self) -> Vec<QualifiedName> {
        self.set.iter().filter_map(|(_, e)| e.as_ref()).flat_map(Expr::tables).collect()
    }
}

/// The `<set clause>`s of an UPDATE [X3.135-1986 §8.11, §8.12] on the one
/// table of `binder`: each column's position and new value (`None` is NULL),
/// in the order written.
fn bind_set_clauses(binder: &Binder<'_>, kind: WriteKind, set: &[SetClause]) -> Result<Vec<(usize, Option<Expr>)>> {
    let rules = match kind {
        WriteKind::UpdatePositioned => SetRules {
            set_function: "[X3.135-1986 §8.11 SR5] a value of a SET clause contains no set function",
            column: "§8.11 SR6",
            assign: UPDATE_POSITIONED_RULE,
        },
        // A DELETE has no SET clauses, so only the UPDATE rules are ever cited.
        WriteKind::UpdateSearched | WriteKind::DeleteSearched | WriteKind::DeletePositioned => SetRules {
            set_function: "[X3.135-1986 §8.12 SR3] a value of a SET clause contains no set function",
            column: "§8.12 SR4",
            assign: UPDATE_SEARCHED_RULE,
        },
    };
    let mut bound_set: Vec<(usize, Option<Expr>)> = Vec::new();
    for sc in set {
        let bound = bind_set_clause(binder, sc, &rules, &bound_set).or_at(sc.at)?;
        bound_set.push(bound);
    }
    Ok(bound_set)
}

/// The rules a `<set clause>` cites: of a set function in its value, of its
/// object column, and of the assignment.
struct SetRules {
    set_function: &'static str,
    column: &'static str,
    assign: AssignRule,
}

/// One `<set clause>` `sc`, after the clauses `done`: its object column's
/// position in the row of the table changed, and its new value.
fn bind_set_clause(binder: &Binder<'_>, sc: &SetClause, rules: &SetRules, done: &[(usize, Option<Expr>)]) -> Result<(usize, Option<Expr>)> {
    let item = binder.items.first().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] an UPDATE binds its table first"))?;
    let (name, desc) = (&item.table, &item.desc);
    let (i, col) = desc
        .column_desc(&sc.column)
        .ok_or_else(|| no_column(name, format!("[X3.135-1986 {}] {name} has no column {}", rules.column, sc.column), &sc.column))?;
    // An updatable view's column is a column of its base table.
    let position = item.position(i);
    if done.iter().any(|(j, _)| *j == position) {
        return Err(rule(format!("[X3.135-1986 {}] column {} is set twice", rules.column, sc.column)));
    }
    let base = match &item.source {
        Source::Base(vb) => vb.base.columns.get(position),
        Source::Table | Source::Derived(_) | Source::Common | Source::System(_) | Source::Unnest(_) => Some(col),
    };
    // [ISO/IEC 9075-2:2023 §14.15 SR5] a generated column, or an identity
    // column always generated, set only to DEFAULT.
    if base.is_some_and(ColumnDesc::always_generated) && !matches!(sc.value, UpdateSource::Default) {
        let msg = format!("[ISO/IEC 9075-2:2023 §14.15 SR5] column {} is generated always, so it is set only to DEFAULT", sc.column);
        return Err(rule(msg));
    }
    if let Some(index) = &sc.element {
        return Ok((position, Some(binder.element_assignment((sc, col), index, rules.set_function)?)));
    }
    let value = match &sc.value {
        // NULL: SQL-86 refuses it for a NOT NULL column [X3.135-1986 §8.11
        // SR8a, §8.12 SR6a]; SQL-92 has no such rule, NOT NULL being a CHECK
        // constraint [ISO/IEC 9075:1992 §11.4 SR9a], checked as the statement
        // ends [§4.10].
        UpdateSource::Null => None,
        // [ISO/IEC 9075:1992 §13.9 GR5] DEFAULT: the default of the column,
        // of the base table if it is a view's [§11.5 GR2].
        // [ISO/IEC 9075-2:2023 §11.5 GR3a] of an identity column, unassigned,
        // so given its generator's next value [§15.14 GR7a]; of a generated
        // column, its expression's value [GR9].
        UpdateSource::Default => match base.and_then(ColumnDesc::identity) {
            Some(identity) => Some(Expr::NextValue(Arc::new(identity.sequence.clone()))),
            None => Some(Expr::Const(base.map_or(Value::Null, |c| c.default_value(binder.user, binder.now)))),
        },
        UpdateSource::Value(v) => {
            // [ISO/IEC 9075:1992 §17.6 GR5w] a `?` takes its object column's type.
            let (e, t) = binder.typed(v, ParameterType::Of(col.data_type.clone()), Scope::Rows(rules.set_function))?;
            check_assignment(col, &t.clone(), rules.assign)?;
            Some(super::datetime::stored(e, t, &col.data_type.clone(), binder.now))
        }
    };
    Ok((position, value))
}

/// An UPDATE or DELETE of `table`, with the SET clauses `set` (none for
/// DELETE) and, for a searched statement, its WHERE condition.
///
/// # Errors
///
/// The table is undefined or not the caller's, or a SET clause or the
/// condition breaks a Syntax Rule.
pub fn bind_write(
    cx: Context<'_>,
    kind: WriteKind,
    table: &TableName,
    set: &[SetClause],
    where_clause: Option<&SearchCondition>,
) -> Result<BoundWrite> {
    let Context { db, auth, .. } = cx;
    let clause = kind.clause();
    let name = qualify(table, cx.schema);
    let target = write_target(db, &name, (kind.read_only_rule(), cx.user), cx.now.noted()).or_at(table.at)?;
    let desc = &target.named;
    // SR1 of each clause: DELETE, or UPDATE on each object column.
    let needed = match kind {
        WriteKind::DeleteSearched | WriteKind::DeletePositioned => Actions { delete: true, ..Actions::default() },
        WriteKind::UpdateSearched | WriteKind::UpdatePositioned => {
            Actions { update: set.iter().filter_map(|s| desc.column(&s.column)).collect(), ..Actions::default() }
        }
    };
    require(db, auth, desc, &needed, &format!("{clause} SR1")).or_at(table.at)?;
    // [X3.135-1986 §8.5 SR3, §8.12 SR5, §8.11 SR7] the scope of the <table name> is the whole statement.
    let item = FromItem::alone(name.clone(), table.at, (desc.clone(), target.source.clone()));
    let visible = (0..desc.columns.len()).map(|k| (0, k)).collect();
    let names = item.row_names();
    let binder = Binder::new(cx, None, (vec![item], names.clone(), visible), BTreeSet::new());

    let cond = where_clause
        .map(|c| {
            binder.search_condition(
                c,
                Scope::Rows("[X3.135-1986 §5.8 SR1] a set function needs an argument source (a <table expression>), which this <search condition> does not have"),
            )
        })
        .transpose()?;

    // [ISO/IEC 9075-2:2023 §6.14 SR1e] an <update source> draws values.
    let bound_set = binder.drawing(true, || bind_set_clauses(&binder, kind, set))?;
    let base = target.base;
    let generated = super::generations(db, &base)?;
    let mut rows = LogicalPlan::Scan { table: base.name.clone(), names, needed: (0..base.columns.len()).collect() };
    if let Some(cond) = and_all(target.cond.into_iter().chain(cond).collect()) {
        rows = LogicalPlan::Filter { cond, input: Box::new(rows) };
    }
    Ok(BoundWrite {
        named: name,
        table: base.name.clone(),
        desc: base,
        set: bound_set,
        rows,
        checks: target.checks,
        auth: auth.to_string(),
        generated,
    })
}
