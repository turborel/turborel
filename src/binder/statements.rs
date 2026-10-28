//! Binding of the statements that use a single table: the updatability of a
//! cursor [X3.135-1986 §8.3 SR4b, §5.25 SR11], single-row SELECT [X3.135-1986
//! §8.10], and searched and positioned UPDATE and DELETE [X3.135-1986 §8.4,
//! §8.5, §8.11, §8.12], with the assignment rules they share. Split from
//! binder.rs (RULES.md R-SIZE-2).

use std::collections::BTreeSet;

use super::views::{Source, ViewCheck, write_target};
use super::{Binder, Bound, Context, FromItem, Scope, bind_query_specification, no_column, qualify, require, rule};
use crate::ast::{
    CursorSpecification, QuerySpecification, SearchCondition, SetClause, TableExpression, TableName, TableReference, UpdateSource,
};
use crate::catalog::Actions;
use crate::catalog::{Catalog, QualifiedName, TableDesc};
use crate::error::{At, Code, Result, SqlError};
use crate::logical::{Expr, LogicalPlan, and_all};
use crate::params::ParameterType;
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
    matches!(te.from.as_slice(), [TableReference::Table(t)] if t.columns.is_none()) && te.group_by.is_empty() && te.having.is_none()
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
/// `col`? A character string in a character string column, a bit string in
/// a bit string one, a number in a numeric one, a datetime in a datetime
/// one of its fields. SQL-86's rules that a string be no longer than the column
/// and only an exact number go to an exact column [X3.135-1986 §8.7 SR6,
/// §8.11 SR8, §8.12 SR6] are Entry SQL's [ISO/IEC 9075:1992 §13.8, §13.9,
/// §13.10, Leveling Rules]: a longer string is checked as it is stored
/// [§9.2 GR3b].
///
/// # Errors
///
/// SQLCODE -103 citing `r` if it cannot.
pub fn check_assignment(col: &crate::catalog::ColumnDesc, t: DataType, r: AssignRule) -> Result<()> {
    let temporal = col.data_type.is_datetime() || t.is_datetime() || col.data_type.is_interval() || t.is_interval();
    let datetime = if temporal { col.data_type.comparable(&t) } else { true };
    if col.data_type.is_character() == t.is_character() && col.data_type.is_bit() == t.is_bit() && datetime {
        return Ok(());
    }
    Err(rule(format!("[ISO/IEC 9075:1992 {}, §9.2 SR2] cannot assign a {t} value to column {} ({})", r.rule, col.name, col.data_type)))
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
    let value = match &sc.value {
        // SR a: NULL only for a column that allows nulls.
        // NULL: SQL-86 refuses it for a NOT NULL column [X3.135-1986 §8.11
        // SR8a, §8.12 SR6a]; SQL-92 has no such rule, NOT NULL being a CHECK
        // constraint [ISO/IEC 9075:1992 §11.4 SR9a], checked as the statement
        // ends [§4.10].
        UpdateSource::Null => None,
        // [ISO/IEC 9075:1992 §13.9 GR5] DEFAULT: the default of the column,
        // of the base table if it is a view's [§11.5 GR2].
        UpdateSource::Default => {
            let base = match &item.source {
                Source::Base(vb) => vb.base.columns.get(position),
                Source::Table | Source::Derived(_) | Source::Common | Source::System(_) => Some(col),
            };
            Some(Expr::Const(base.map_or(Value::Null, |c| c.default_value(binder.user, binder.now))))
        }
        UpdateSource::Value(v) => {
            // [ISO/IEC 9075:1992 §17.6 GR5w] a `?` takes its object column's type.
            let (e, t) = binder.typed(v, ParameterType::Of(col.data_type), Scope::Rows(rules.set_function))?;
            check_assignment(col, t, rules.assign)?;
            Some(super::datetime::stored(e, t, col.data_type, binder.now))
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
    let target = write_target(db, &name, (kind.read_only_rule(), cx.user)).or_at(table.at)?;
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

    let bound_set = bind_set_clauses(&binder, kind, set)?;
    let base = target.base;
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
    })
}
