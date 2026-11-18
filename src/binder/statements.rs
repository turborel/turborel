//! Binding of the statements that use a single table: the updatability of a
//! cursor [X3.135-1986 §8.3 SR4b, §5.25 SR11], single-row SELECT [X3.135-1986
//! §8.10], and searched and positioned UPDATE and DELETE [X3.135-1986 §8.4,
//! §8.5, §8.11, §8.12], with the assignment rules they share.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::portion::Portioned;
use super::views::{Source, ViewCheck, write_target};
use super::{Binder, Bound, Context, FromItem, Scope, bind_query_specification, no_column, qualify, require, rule};
use crate::ast::{
    CursorSpecification, Portion, QuerySpecification, SearchCondition, SetClause, SystemTimeRead, TableExpression, TableName,
    TableReference, UpdateSource,
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
    // [ISO/IEC 9075:1992 §7.10 SR4] no set operation; [ISO/IEC
    // 9075-2:2023 §7.17 SR7] no OFFSET or FETCH FIRST.
    let s = c.query.updatable_spec()?;
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

/// [ISO/IEC 9075-2:2023 §14.8 SR9] Whether the `<table reference>` of the
/// updatable cursor specification `c` is ONLY.
#[must_use]
pub fn updatable_only(c: &CursorSpecification) -> bool {
    updatable(c).and_then(|s| s.table_expression.from.first()?.named()).is_some_and(|n| n.only)
}

/// [X3.135-1986 §5.25 SR11c, SR11e] one `<table reference>`, naming a
/// table [ISO/IEC 9075:1992 §7.5 SR10, §7.9 SR12c], and no GROUP BY or
/// HAVING; read FOR SYSTEM_TIME only AS OF CURRENT_TIMESTAMP [ISO/IEC
/// 9075-2:2023 §7.16 SR22d].
fn single_ungrouped_table(te: &TableExpression) -> bool {
    let [TableReference::Table(t)] = te.from.as_slice() else { return false };
    t.columns.is_none() && t.system_time.as_deref().is_none_or(SystemTimeRead::current) && te.group_by.is_none() && te.having.is_none()
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
    /// The clause and Syntax Rule, cited in full, e.g. "ISO/IEC 9075:1992
    /// §13.8 SR6".
    pub rule: &'static str,
}

/// The assignment rule of INSERT.
pub const INSERT_RULE: AssignRule = AssignRule { rule: "ISO/IEC 9075:1992 §13.8 SR6" };
/// The assignment rule of a searched UPDATE.
pub const UPDATE_SEARCHED_RULE: AssignRule = AssignRule { rule: "ISO/IEC 9075:1992 §13.10 SR5" };
/// The assignment rule of a positioned UPDATE.
pub const UPDATE_POSITIONED_RULE: AssignRule = AssignRule { rule: "ISO/IEC 9075:1992 §13.9 SR10" };

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
    Err(rule(format!("[{}; ISO/IEC 9075:1992 §9.2 SR2] cannot assign a {t} value to column {} ({})", r.rule, col.name, col.data_type)))
}

/// [ISO/IEC 9075:1992 §9.2 SR2] Can a value of type `t` be stored in a
/// target of type `target`? A character string in a character string, a
/// binary string in a binary string [ISO/IEC 9075-2:2023 §9.2 SR3], a
/// number in a number alone, a truth value in a Boolean alone [ISO/IEC
/// 9075-2:2023 §4.6.2], a datetime in a datetime of its fields, an
/// interval in a comparable interval.
/// \[EXTENSION\] Where either is a type beyond the standard, which SR2
/// does not speak of (RULES.md D53): a value comparable with the target,
/// as its kind says, or a character string, if the target's kind takes
/// one (`SqlType::assigns_text`). [ISO/IEC 9075-2:2023 §9.2 SR2d, SR2e]
/// An array in an array whose element type its elements are assignable
/// to, a multiset in a multiset so [§4.12.4], a row in a row of its
/// degree, field by field. [§4.9.5, §4.13, §9.2
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
    if let (DataType::Multiset(a), DataType::Multiset(b)) = (target, t) {
        return assignable(&a.element, &b.element);
    }
    if let (DataType::Row(a), DataType::Row(b)) = (target, t) {
        return a.fields.len() == b.fields.len() && a.fields.iter().zip(&b.fields).all(|(x, y)| assignable(&x.data_type, &y.data_type));
    }
    let composite = |either: &DataType| matches!(either, DataType::Array(_) | DataType::Multiset(_) | DataType::Row(_));
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
    // [ISO/IEC 9075-2:2023 §4.5.2, §4.6.2] a number only to a numeric site,
    // so not to a Boolean one, nor a truth value to a numeric one.
    let numeric = target.is_numeric() == t.is_numeric();
    target.is_character() == t.is_character() && target.is_binary() == t.is_binary() && numeric && datetime
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
            WriteKind::UpdateSearched => "X3.135-1986 §8.12 SR2",
            WriteKind::UpdatePositioned => "X3.135-1986 §8.11 SR3",
            WriteKind::DeleteSearched => "X3.135-1986 §8.5 SR2",
            WriteKind::DeletePositioned => "X3.135-1986 §8.4 SR3",
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
    /// [ISO/IEC 9075-2:2023 §15.8 GR9, §15.14 GR12] Of a statement FOR
    /// PORTION OF a period, over the old row: the end of its part before
    /// the portion and the start of its part after, each null if none.
    pub portion: Option<super::portion::Pieces>,
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
            column: "X3.135-1986 §8.11 SR6",
            assign: UPDATE_POSITIONED_RULE,
        },
        // A DELETE has no SET clauses, so only the UPDATE rules are ever cited.
        WriteKind::UpdateSearched | WriteKind::DeleteSearched | WriteKind::DeletePositioned => SetRules {
            set_function: "[X3.135-1986 §8.12 SR3] a value of a SET clause contains no set function",
            column: "X3.135-1986 §8.12 SR4",
            assign: UPDATE_SEARCHED_RULE,
        },
    };
    bind_set_list(binder, set, &rules)
}

/// The `<set clause list>` `set` on the one table of `binder`, each clause
/// bound after those before it: of a `<mutated set clause>` of a column an
/// earlier one sets, that one deleted [ISO/IEC 9075-2:2023 §14.15 SR7c ii].
pub(super) fn bind_set_list(binder: &Binder<'_>, set: &[SetClause], rules: &SetRules) -> Result<Vec<(usize, Option<Expr>)>> {
    let mut done: Vec<(usize, Option<Expr>)> = Vec::new();
    let mut mutated: Vec<usize> = Vec::new();
    for sc in set {
        let bound = binder.hosting(true, || bind_set_clause(binder, sc, rules, (&done, &mutated))).or_at(sc.at)?;
        if !sc.attributes.is_empty() {
            done.retain(|(j, _)| *j != bound.0);
            mutated.push(bound.0);
        }
        done.push(bound);
    }
    Ok(done)
}

/// The rules a `<set clause>` cites: of a set function in its value, of its
/// object column, and of the assignment.
pub(super) struct SetRules {
    /// The message of a set function in a value, its rule first.
    pub(super) set_function: &'static str,
    /// The rule of the object column, cited in full.
    pub(super) column: &'static str,
    /// The rule of the assignment.
    pub(super) assign: AssignRule,
}

/// One `<set clause>` `sc`, after the clauses `done`, of which those of
/// the columns at `mutated` are mutated set clauses: its object column's
/// position in the row of the table changed, and its new value.
fn bind_set_clause(
    binder: &Binder<'_>,
    sc: &SetClause,
    rules: &SetRules,
    (done, mutated): (&[(usize, Option<Expr>)], &[usize]),
) -> Result<(usize, Option<Expr>)> {
    let item = binder.items.first().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] an UPDATE binds its table first"))?;
    let (name, desc) = (&item.table, &item.desc);
    let (i, col) = desc
        .column_desc(&sc.column)
        .ok_or_else(|| no_column(name, format!("[{}] {name} has no column {}", rules.column, sc.column), &sc.column))?;
    // An updatable view's column is a column of its base table.
    let position = item.position(i);
    // [ISO/IEC 9075-2:2023 §14.15 SR7c ii, SR8] a column set twice, but by
    // mutated set clauses, the later of which takes the earlier's value.
    let folded = !sc.attributes.is_empty() && mutated.contains(&position);
    if !folded && done.iter().any(|(j, _)| *j == position) {
        return Err(rule(format!("[{}] column {} is set twice", rules.column, sc.column)));
    }
    let table = match &item.source {
        Source::Base(vb) => &vb.base,
        Source::Table | Source::Derived(_) | Source::Common | Source::System(_) | Source::Unnest(_) | Source::Argument => desc,
    };
    let base = table.columns.get(position);
    settable(table, base, sc)?;
    if let Some(index) = &sc.element {
        return Ok((position, Some(binder.element_assignment((sc, col), index, rules.set_function)?)));
    }
    if !sc.attributes.is_empty() {
        let at = binder.note(item, i)?;
        let earlier = done.iter().find(|(j, _)| folded && *j == position).map(|(_, e)| e.clone().unwrap_or(Expr::Const(Value::Null)));
        let current = earlier.unwrap_or(Expr::Column(at));
        return Ok((position, Some(binder.mutated_value((sc, at, &col.data_type), &current, rules.set_function)?)));
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
            // [ISO/IEC 9075-2:2023 §9.2 GR2] by an appropriate cast.
            if let Some(cast) = super::assignment_cast(&*binder.db.catalog, &t, &col.data_type) {
                Some(binder.user_cast_invocation(cast, e)?.0)
            } else {
                check_assignment(col, &t.clone(), rules.assign)?;
                Some(super::datetime::stored(e, t, &col.data_type.clone(), binder.now))
            }
        }
    };
    Ok((position, value))
}

/// [ISO/IEC 9075-2:2023 §14.15 SR3, SR5] That `sc` may set the column
/// `base` of `table`: not its self-referencing column, nor a system-time
/// period column, and of a generated column, or an identity column always
/// generated, only to DEFAULT.
fn settable(table: &TableDesc, base: Option<&ColumnDesc>, sc: &SetClause) -> Result<()> {
    // [ISO/IEC 9075-2:2023 §14.15 SR3] no self-referencing column set.
    if let Some(typed) = &table.typed
        && base.is_some_and(|c| c.name == typed.self_referencing)
    {
        let msg = format!(
            "[ISO/IEC 9075-2:2023 §14.15 SR3] column {} is the self-referencing column of {}, so no <set clause> sets it",
            sc.column, table.name
        );
        return Err(rule(msg));
    }
    if base.is_some_and(|c| c.system_time().is_some()) {
        let msg =
            format!("[ISO/IEC 9075-2:2023 §14.15 SR3] column {} is a system-time period column, so no <set clause> sets it", sc.column);
        return Err(rule(msg));
    }
    // [ISO/IEC 9075-2:2023 §14.15 SR5] a generated column, or an identity
    // column always generated, set only to DEFAULT.
    if base.is_some_and(ColumnDesc::always_generated) && (!matches!(sc.value, UpdateSource::Default) || !sc.attributes.is_empty()) {
        let msg = format!("[ISO/IEC 9075-2:2023 §14.15 SR5] column {} is generated always, so it is set only to DEFAULT", sc.column);
        return Err(rule(msg));
    }
    Ok(())
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
    // [ISO/IEC 9075-2:2023 §14.9 SR8, §14.14 SR8] ONLY of a table that is
    // not typed is the table's name alone; of a typed table, its own rows,
    // which are all this binds (`bind_member_write` binds a subtable's).
    (table, only): (&TableName, bool),
    set: &[SetClause],
    where_clause: Option<&SearchCondition>,
) -> Result<BoundWrite> {
    bind_searched_write(cx, kind, (table, only, None), set, where_clause)
}

/// The table a searched UPDATE or DELETE changes: its name, whether ONLY
/// is written, and FOR PORTION OF, if written.
pub type SearchedTarget<'a> = (&'a TableName, bool, Option<&'a Portion>);

/// `bind_write` of a searched UPDATE or DELETE FOR `portion` of a period
/// of its table, if written (`binder/portion.rs`).
///
/// # Errors
///
/// As `bind_write`, and of FOR PORTION OF \[§14.9 SR7a, §14.14 SR7a\].
pub fn bind_searched_write(
    cx: Context<'_>,
    kind: WriteKind,
    (table, only, portion): SearchedTarget<'_>,
    set: &[SetClause],
    where_clause: Option<&SearchCondition>,
) -> Result<BoundWrite> {
    let w = bind_write_of(cx, kind, (table, None, portion), set, where_clause)?;
    if only {
        let clause =
            if matches!(kind, WriteKind::DeleteSearched | WriteKind::DeletePositioned) { "§14.9 AR1b ii" } else { "§14.14 AR1b ii" };
        super::hierarchy::only_allowed(cx.db, cx.who(), &w.desc, clause).or_at(table.at)?;
    }
    Ok(w)
}

/// [ISO/IEC 9075-2:2023 §14.9 GR4, §14.14 GR6; §15.7, §15.13] The UPDATE
/// or DELETE of a typed table `table`, named without ONLY, as it changes
/// the rows of its proper subtable `member`: their superrows in it, read
/// and written by its columns and privileges, which are the member's first
/// columns, the rows the member's own.
///
/// # Errors
///
/// As `bind_write`.
pub fn bind_member_write(
    cx: Context<'_>,
    kind: WriteKind,
    (table, member): (&TableName, &QualifiedName),
    set: &[SetClause],
    where_clause: Option<&SearchCondition>,
) -> Result<BoundWrite> {
    bind_write_of(cx, kind, (table, Some(member), None), set, where_clause)
}

/// `bind_write` of `table`'s rows, or of its proper subtable `member`'s.
fn bind_write_of(
    cx: Context<'_>,
    kind: WriteKind,
    (table, member, portion): (&TableName, Option<&QualifiedName>, Option<&Portion>),
    set: &[SetClause],
    where_clause: Option<&SearchCondition>,
) -> Result<BoundWrite> {
    let Context { db, auth, .. } = cx;
    let clause = kind.clause();
    let name = qualify(table, cx.schema);
    let target = write_target(db, &name, (kind.read_only_rule(), cx.user), cx.now.noted()).or_at(table.at)?;
    let desc = &target.named;
    let portioned = portion.map(|p| Portioned::of((desc, table), (kind, p), set, where_clause)).transpose()?;
    let (set, where_clause) = portioned.as_ref().map_or((set, where_clause), |p| (p.set.as_slice(), Some(&p.cond)));
    // SR1 of each clause: DELETE, or UPDATE on each object column.
    let needed = match kind {
        WriteKind::DeleteSearched | WriteKind::DeletePositioned => Actions { delete: true, ..Actions::default() },
        WriteKind::UpdateSearched | WriteKind::UpdatePositioned => {
            Actions { update: set.iter().filter_map(|s| desc.column(&s.column)).collect(), ..Actions::default() }
        }
    };
    require(db, cx.who(), desc, &needed, &format!("X3.135-1986 {clause} SR1")).or_at(table.at)?;
    // [X3.135-1986 §8.5 SR3, §8.12 SR5, §8.11 SR7] the scope of the <table name> is the whole statement.
    let item = FromItem::alone(name.clone(), table.at, (desc.clone(), target.source.clone()));
    let visible = (0..desc.columns.len()).map(|k| (0, k)).collect();
    let names = item.row_names();
    let binder = Binder::new(cx, None, (vec![item], names.clone(), visible), BTreeSet::new());

    let rows_scope = Scope::Rows(
        "[X3.135-1986 §5.8 SR1] a set function needs an argument source (a <table expression>), which this <search condition> does not have",
    );
    // [§14.9 SR7a ii, iii; §14.14 SR7a iii, iv] the points in time judged first.
    let pieces = portioned.as_ref().map(|p| p.bind(&binder, rows_scope)).transpose()?;
    let cond = where_clause.map(|c| binder.search_condition(c, rows_scope)).transpose()?;

    // [ISO/IEC 9075-2:2023 §6.14 SR1e] an <update source> draws values.
    let bound_set = binder.drawing(true, || bind_set_clauses(&binder, kind, set))?;
    let base = match member {
        Some(m) => db.catalog.table(m).cloned().ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] subtable {m} is not there")))?,
        None => target.base,
    };
    let generated = super::written_generations(db, &base, Some(set))?;
    let names = if member.is_some() { base.columns.iter().map(|c| c.name.clone()).collect() } else { names };
    let mut rows = LogicalPlan::Scan { table: base.name.clone(), names, needed: (0..base.columns.len()).collect(), located: false };
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
        portion: pieces,
    })
}
