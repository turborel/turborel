//! Binding: AST -> logical plan.
//!
//! The binder enforces the Syntax Rules that need the catalog: names exist
//! [X3.135-1986 §5.4 SR5], column references are unambiguous [X3.135-1986 §5.7 SR3], types are
//! comparable [X3.135-1986 §5.11 SR1], privileges are held [X3.135-1986 §5.25 SR1], grouped queries
//! reference only grouping columns and columns functionally dependent on them outside set functions
//! ([X3.135-1986 §5.25 SR7, §5.23 SR1; ISO/IEC 9075-2:2023 §6.7 SR8]),
//! set operations' operands correspond [ISO/IEC 9075:1992 §7.10 SR9], and sort keys are valid
//! [X3.135-1986 §8.3 SR6].
//!
//! It then builds the plan exactly as the General Rules describe it, with no
//! optimization: the FROM product, then WHERE, GROUP BY, HAVING, the select
//! list, DISTINCT, the set operations, ORDER BY ([X3.135-1986 §5.19 GR1, §5.25, §8.3;
//! ISO/IEC 9075:1992 §7.10]). Making it fast is the optimizer's and the physical planner's job.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;

use crate::ast::{ColumnSpecification, TableName, ValueExpression};
use crate::catalog::{Actions, ColumnDesc, Horizon, Principal, QualifiedName, Who};
use crate::db::State;
use crate::error::{At, Code, Result, SqlError, Subject};
use crate::logical::{AggCall, Expr, LogicalPlan};
use crate::params::Params;
use crate::settings::Own;
use crate::value::DataType;
use crate::value::collation::CollationRef;
use crate::value::datetime::Clocked;

mod appended;
mod arguments;
mod arithmetic;
mod case;
mod collation;
mod composite;
mod context;
mod counterparts;
pub(crate) use composite::key_operand;
mod cursor;
mod datetime;
mod dependencies;
pub(crate) use dependencies::{DescribedKey, described_key};
mod interval;
mod invocation;
mod json;
mod key_expression;
mod listagg;
mod lobs;
mod merge;
mod multisets;
pub(crate) use datetime::cast_is_valid;
pub use datetime::{function_type as datetime_function_type, literal_value as datetime_literal};
pub use interval::{displacement_of, is_displacement};
mod distribution;
mod embedding;
mod expression;
mod filter;
mod from;
mod generated;
mod greatest;
mod grouping_sets;
mod hierarchy;
mod hybrid_search;
pub use hierarchy::{family_in, member_plan};
mod hypothetical;
mod invariant;
mod limit;
mod natural;
mod noted;
mod order_by;
mod parameters;
mod partitioned;
mod pattern;
mod periods;
mod portion;
mod predicates;
mod psm;
mod ptf;
mod purpose;
mod query;
mod recursive;
mod referenceable;
mod referencing;
mod required;
mod routines;
mod search_cycle;
mod sequences;
mod set_functions;
mod set_operations;
mod statements;
mod strata;
mod structured;
mod subquery;
mod system_time;
mod triggers;
mod trim;
mod underlying;
mod unnest;
mod values;
mod views;
mod window;
mod with;

pub use counterparts::defaulted_columns;
pub use cursor::bind_cursor;
use from::FromItem;
pub use generated::{bind_generation, generations, noting_generation, written_generations};
pub use invocation::{
    Candidate, FunctionBody, alike, bind_function_body, definition_of, functions_named, grantably_held, held, resolve_procedure,
    routine_definition, sql_statement,
};
pub use key_expression::bind_key;
pub use merge::{BoundAction, BoundMerge, BoundWhen, bind_merge};
pub use parameters::{assigned_argument, bind_argument};
pub(crate) use psm::{Frame, bind_program};
use query::bind_query_specification;
pub use set_operations::{ObjectColumn, bind_insert_query};
pub(crate) use statements::assignable;
pub use statements::{
    AssignRule, BoundWrite, INSERT_RULE, SearchedTarget, UPDATE_POSITIONED_RULE, UPDATE_SEARCHED_RULE, WriteKind, bind_member_write,
    bind_searched_write, bind_select, bind_write, check_assignment, updatable, updatable_in, updatable_only,
};
pub(crate) use structured::assignment_cast;
use subquery::Outer;
pub use triggers::bind_when;
pub use underlying::underlying;
pub use values::{value_specification, value_specification_as};
pub use views::{BoundView, ViewCheck, WriteTarget, bind_view, table_columns, view_degree, write_target};
pub(crate) use views::{once_each, system_view_columns, view_uses};
pub(crate) use with::Queries;

/// A column of a cursor's result.
#[derive(Debug, Clone)]
pub struct OutColumn {
    /// [X3.135-1986 §5.25 SR9] named only when the select-list item is a single
    /// `<column specification>`, or [ISO/IEC 9075:1992 §7.9 SR9a] has an `<as
    /// clause>`; a set operation's as both operands name it [§7.10 SR10b i].
    pub name: Option<String>,
    /// [ISO/IEC 9075-2:2023 §9.5] Its collation, none being the character
    /// set's.
    pub collation: Option<QualifiedName>,
    /// The column's type.
    pub data_type: DataType,
    /// [X3.135-1986 §5.25 SR10] constrained NOT NULL only if a named column of a NOT NULL column.
    pub not_null: bool,
    /// For a named column, its position in the FROM row (used by ORDER BY,
    /// and by an updatable cursor to find the column in its table).
    pub(crate) origin: Option<usize>,
}

/// A bound query: its plan, as the General Rules describe it, and its columns.
pub struct Bound {
    /// The logical plan.
    pub plan: LogicalPlan,
    /// The result's columns, in order.
    pub columns: Vec<OutColumn>,
}

/// What a statement is bound with: the database, the authorization
/// identifier it runs under, the parameters of its procedure, and what it
/// is bound for.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    /// The database state.
    pub db: &'a State,
    /// The `<module authorization identifier>` [X3.135-1986 §5.4 SR2b]:
    /// whose privileges the query is bound with; a view's owner, within the
    /// view.
    pub auth: &'a str,
    /// [ISO/IEC 9075-2:2023 §4.42.1.2] The current role name, of a cell
    /// whose `auth` is a user identifier: the session's after SET ROLE;
    /// none within a view, nor of a definition, bound as its owner alone.
    pub role: Option<&'a str>,
    /// The schema an unqualified name is of [X3.135-1986 §5.4 SR2]: the
    /// session's default [ISO/IEC 9075:1992 §4.30, §16.2], `auth`'s unless
    /// SET SCHEMA set another; a view's owner's, within the view.
    pub schema: &'a str,
    /// [ISO/IEC 9075:1992 §4.26, §6.2 GR9; ISO/IEC 9075-2:2023 §6.4 GR6
    /// to GR9] Whose the statement is, what CURRENT_USER, CURRENT_ROLE,
    /// SESSION_USER and SYSTEM_USER give: the statement's, within a view it
    /// reads too.
    pub user: Principal<'a>,
    /// The procedure's parameters [X3.135-1986 §7.3].
    pub params: &'a Params,
    /// What the query is bound for.
    pub purpose: Purpose,
    /// [ISO/IEC 9075-2:2023 §9.15 SR4c] The session's collation, of a
    /// statement it applies to: none of a view, a constraint or a default,
    /// which keep what they were defined with.
    pub collation: Option<&'a CollationRef>,
    /// The columns it sees of the tables it reads: a view's or a CHECK's,
    /// as they were defined [ISO/IEC 9075:1992 §11.11 GR2, note].
    pub horizon: &'a Horizon,
    /// Its session's own settings, which the system tables show (`system.rs`).
    pub own: &'a Own,
    /// Whether it may read the Definition Schema: only a view of the
    /// Information Schema does [ISO/IEC 9075:1992 §21.1].
    pub definitions: bool,
    /// Where to note the columns of named tables and views it references
    /// [ISO/IEC 9075:1992 §21.3.12, §21.3.18], if anywhere.
    pub uses: Option<&'a Uses>,
    /// Where to note, with `uses`, the columns each item of its select list
    /// references, if anywhere: of the query, not of its subqueries.
    pub selected: Option<&'a Selected>,
    /// Of a constraint, where to note what its REFERENCES depend on
    /// [ISO/IEC 9075:1992 §11.9 AR1], if anywhere: if nowhere, each table
    /// it reads needs some.
    pub referencing: Option<&'a Referencing>,
    /// [ISO/IEC 9075:1992 §6.8 GR3, §4.5.1] The instant its statement runs
    /// at, and its SQL-session's time zone displacement.
    pub now: Clocked<'a>,
    /// [ISO/IEC 9075-2:2023 §6.14 SR1a, SR1c] Whether the select list of
    /// its `<query specification>`, or the rows of its `<table value
    /// constructor>`, may draw values of sequence generators: of the whole
    /// query of a cursor, of a SELECT INTO, or of an INSERT. Not of its
    /// subqueries.
    pub draws: bool,
    /// [ISO/IEC 9075-2:2023 §7.17 SR3i] The query names of the `<with
    /// clause>`s it is in the scope of, if any: of none at the start of a
    /// statement, nor in a view, which is bound where it is defined.
    pub(crate) queries: Option<&'a Queries<'a>>,
    /// Where to note the specific names of the SQL-invoked routines it
    /// invokes [ISO/IEC 9075-2:2023 §11.60 GR1], if anywhere.
    pub invoked: Option<&'a Invoked>,
    /// Where to note the sequence generators it draws from, if anywhere.
    pub drawn: Option<&'a DrawnFrom>,
    /// [ISO/IEC 9075-2:2023 §7.17 SR25] Whether the rows of its `<table value
    /// constructor>` may invoke a function that possibly modifies SQL-data:
    /// of an INSERT's, or a program's value (`Binder::hosting`).
    pub hosts: bool,
    /// [ISO/IEC 9075-2:2023 §9.24] The database and transaction of the
    /// statement it is bound in, if one runs: where a PTF describe
    /// component procedure reads SQL-data. None as a definition is bound.
    pub compile: Option<&'a Compile>,
}

pub use noted::{DrawnFrom, Invoked, Selected, Uses};
pub use ptf::Compile;
pub use purpose::Purpose;
pub use referencing::Referencing;
pub use required::require;

/// [X3.135-1986 §5.4 SR2] a `<table name>` without an `<authorization identifier>` implicitly
/// has the one of the enclosing `<schema>` or `<module>`. In direct invocation `auth` is the
/// session's default schema (`Context::schema`): its authorization identifier's, unless SET
/// SCHEMA set another.
pub fn qualify(name: &TableName, auth: &str) -> QualifiedName {
    QualifiedName { auth: name.auth.clone().unwrap_or_else(|| auth.to_string()), ident: name.ident.clone() }
}

/// [ISO/IEC 9075:1992 §16.5] A `<value expression>` of no table, as SET
/// TIME ZONE gives one: bound, and its type.
///
/// # Errors
///
/// It does not bind.
pub fn bind_value(cx: Context<'_>, v: &ValueExpression) -> Result<(Expr, DataType)> {
    let binder = Binder::new(cx, None, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
    binder.value_expression(v, Scope::Rows("[ISO/IEC 9075:1992 §16.5] SET TIME ZONE has no set function"))
}

fn rule(msg: impl Into<String>) -> SqlError {
    SqlError::new(Code::SyntaxRule, msg)
}

/// [ISO/IEC 9075:1992 §11.9 SR6] A constraint's query is `what`, which
/// `rules` make possibly non-deterministic.
fn nondeterministic(rules: &str, what: &str) -> SqlError {
    rule(format!(
        "[ISO/IEC 9075:1992 §11.9 SR6, {rules}] a CHECK condition shall not contain a query that is possibly non-deterministic: {what}"
    ))
}

/// A Syntax Rule violation `message`: `table` has no column `column`,
/// which it names, and the table, if it is a named one [ISO/IEC 9075:1992
/// §18.1 GR3g].
pub(crate) fn no_column(table: &QualifiedName, message: String, column: &str) -> SqlError {
    let e = rule(message);
    if table.auth.is_empty() {
        return e;
    }
    e.about(Subject { column: Some(column.to_owned()), ..Subject::table(table.parts()) })
}

// ---------------------------------------------------------------------------
// Scopes
// ---------------------------------------------------------------------------

/// The grouping columns of a grouped query, and the set functions found so far.
struct Grouping {
    /// The columns of the rows grouped that it groups by: of the FROM row,
    /// or, of grouping sets, of the rows `grouping_sets::Sets` makes.
    columns: Vec<usize>,
    /// What each is grouped by, if any is not the default collation.
    collations: Vec<Option<CollationRef>>,
    aggs: RefCell<Vec<AggCall>>,
    /// The collation each of `aggs` compares its result by, if any.
    collated: RefCell<Vec<Option<CollationRef>>>,
    /// [ISO/IEC 9075-2:2023 §7.13 SR17] Its grouping sets, of a GROUP BY
    /// not simple; none of a primitive one [SR10, SR11].
    sets: Option<grouping_sets::Sets>,
    /// [ISO/IEC 9075-2:2023 §6.7 SR8] Of each grouping set, the FROM-row
    /// columns a column referenced in a group, no grouping column, must be
    /// functionally dependent on (`binder/invariant.rs`).
    determinants: Vec<Vec<usize>>,
    /// [ISO/IEC 9075-2:2023 §9.12] Of each grouping column of a structured
    /// type ordered by MAP, of a primitive GROUP BY, its position and its
    /// value given its comparison key (RULES.md D81).
    keyed: Vec<(usize, Expr)>,
}

/// What a column reference means where it appears.
#[derive(Clone, Copy)]
enum Scope<'g> {
    /// One row of the FROM/WHERE result. The text is the rule broken if a set
    /// function appears here.
    Rows(&'static str),
    /// One group. The text is the rule broken by referencing a column that is
    /// neither a grouping column nor inside a set function.
    Groups(&'g Grouping, &'static str),
    /// One row of an ungrouped query, not aggregated, in its select list. A
    /// set function over an outer reference to it, in a subquery there,
    /// makes it aggregated [ISO/IEC 9075:1992 §6.5 SR5a, §7.9 SR7]
    /// (`Binder::aggregated`).
    Select,
}

/// [X3.135-1986 §5.25] The rule a set function in the select list of a
/// query not aggregated would break, were it not what aggregates it.
const NOT_AGGREGATED: &str = "[X3.135-1986 §5.25] a set function makes the query aggregated";

struct Binder<'a> {
    db: &'a State,
    params: &'a Params,
    auth: &'a str,
    /// The current role name (`Context::role`).
    role: Option<&'a str>,
    /// The schema of unqualified names (`Context::schema`).
    schema: &'a str,
    items: Vec<FromItem>,
    /// Display names of the FROM row's columns, e.g. "E.SALARY".
    names: Vec<String>,
    /// Every FROM column the query references: what the storage must deliver.
    used: RefCell<BTreeSet<usize>>,
    /// The query this one is a subquery of, if it is one (`binder/subquery.rs`).
    outer: Option<&'a Outer<'a>>,
    /// The session's collation, if it applies (`Context::collation`).
    session_collation: Option<&'a CollationRef>,
    /// The collations of each subquery's columns, as each is bound
    /// (`binder/collation.rs`).
    subquery_collations: RefCell<collation::SubqueryCollations>,
    /// What the query is bound for.
    purpose: Purpose,
    /// The columns of `items` it sees unqualified, in order (`From::visible_of`).
    visible: Vec<(usize, usize)>,
    /// The columns it sees of the tables it reads (`Context::horizon`).
    horizon: &'a Horizon,
    /// Its session's own settings (`Context::own`).
    own: &'a Own,
    /// Whose the statement is (`Context::user`).
    user: Principal<'a>,
    /// Whether it may read the Definition Schema (`Context::definitions`).
    definitions: bool,
    /// Where it notes the columns it references (`Context::uses`).
    uses: Option<&'a Uses>,
    /// Where it notes those of each item of its select list
    /// (`Context::selected`).
    selected: Option<&'a Selected>,
    /// Where it notes what its REFERENCES depend on (`Context::referencing`).
    referencing: Option<&'a Referencing>,
    /// Set when a subquery of its select list has a set function over an
    /// outer reference to it, while it is not aggregated: it is then bound
    /// again, aggregated [ISO/IEC 9075:1992 §7.9 SR7].
    aggregated: Cell<bool>,
    /// Its statement's instant and time zone (`Context::now`).
    now: Clocked<'a>,
    /// Whether its select list, or its rows, may draw values of sequence
    /// generators (`Context::draws`).
    draws: bool,
    /// Whether what it binds now may draw them: an item of that select
    /// list, a row of those, or the source of a SET clause, not in a CASE,
    /// a search condition or a set function [ISO/IEC 9075-2:2023 §6.14
    /// SR1, SR2] (`binder/sequences.rs`).
    drawing: Cell<bool>,
    /// [ISO/IEC 9075-2:2023 §4.35.2, §7.17 SR25] Whether what it binds now
    /// may invoke a function that possibly modifies SQL-data: the source of
    /// a SET clause, a row of an INSERT's or a MERGE's VALUES, an argument
    /// of CALL, or a program's value; not in a subquery, nor a CASE
    /// abbreviation or a simple CASE's operand [§6.12 SR1a, SR2a]
    /// (`binder/invocation.rs`).
    hosting: Cell<bool>,
    /// [ISO/IEC 9075-2:2023 §9.14 SR2f] Whether a row it binds now holds
    /// the sort keys of an ARRAY_AGG or a LISTAGG after its first field,
    /// each of a structured type given its key (`binder/composite.rs`).
    sorting: Cell<bool>,
    /// The query names in scope (`Context::queries`).
    queries: Option<&'a Queries<'a>>,
    /// What a PTF describe component procedure reads SQL-data with (`Context::compile`).
    compile: Option<&'a Compile>,
    /// Where it notes the routines it invokes (`Context::invoked`).
    invoked: Option<&'a Invoked>,
    /// Where it notes the sequence generators it draws from (`Context::drawn`).
    drawn: Option<&'a DrawnFrom>,
    /// The window functions of its select list (`binder/window.rs`).
    windowing: RefCell<Option<window::Windowing>>,
    /// The names of its result's columns, while its sort keys are bound
    /// (`binder/order_by.rs`).
    sort_names: RefCell<order_by::SortNames>,
    /// Its FROM and WHERE, while its grouped clauses are bound
    /// (`binder/invariant.rs`).
    rows: RefCell<invariant::Rows>,
    /// Of the `<row pattern measures>` and `<row pattern definition search
    /// condition>`s of row pattern recognition, what their row pattern
    /// column references read (`binder/pattern.rs`).
    recognizing: Option<Box<pattern::Recognizing>>,
}

/// The FROM row a binder sees: its items, the display names of its
/// columns, and those seen unqualified (`From::visible_of`).
type Row = (Vec<FromItem>, Vec<String>, Vec<(usize, usize)>);

impl<'a> Binder<'a> {
    /// A binder of a query bound with `cx`, a subquery of `outer`'s if one
    /// is given, over the FROM row `row`, whose columns `used` it reads.
    fn new(cx: Context<'a>, outer: Option<&'a Outer<'a>>, (items, names, visible): Row, used: BTreeSet<usize>) -> Binder<'a> {
        let (used, aggregated) = (RefCell::new(used), Cell::new(false));
        Binder {
            db: cx.db,
            params: cx.params,
            auth: cx.auth,
            role: cx.role,
            schema: cx.schema,
            items,
            names,
            used,
            outer,
            session_collation: cx.collation,
            subquery_collations: RefCell::new(Vec::new()),
            purpose: cx.purpose,
            visible,
            horizon: cx.horizon,
            own: cx.own,
            user: cx.user,
            definitions: cx.definitions,
            uses: cx.uses,
            selected: cx.selected,
            referencing: cx.referencing,
            aggregated,
            now: cx.now,
            draws: cx.draws && !cx.purpose.constrains(),
            drawing: Cell::new(false),
            hosting: Cell::new(false),
            sorting: Cell::new(false),
            queries: cx.queries,
            compile: cx.compile,
            invoked: cx.invoked,
            drawn: cx.drawn,
            windowing: RefCell::new(None),
            sort_names: RefCell::new(order_by::SortNames::default()),
            rows: RefCell::new(invariant::Rows::default()),
            recognizing: None,
        }
    }

    /// What its subqueries are bound with: what it is.
    fn context(&self) -> Context<'a> {
        let (db, auth, role, schema, user, params) = (self.db, self.auth, self.role, self.schema, self.user, self.params);
        let (purpose, horizon, own, definitions) = (self.purpose, self.horizon, self.own, self.definitions);
        let (referencing, now, collation) = (self.referencing, self.now, self.session_collation);
        Context {
            db,
            auth,
            role,
            schema,
            user,
            params,
            purpose,
            collation,
            horizon,
            own,
            definitions,
            uses: self.uses,
            selected: None,
            referencing,
            now,
            draws: false,
            queries: self.queries,
            invoked: self.invoked,
            drawn: self.drawn,
            hosts: false,
            compile: self.compile,
        }
    }
}

impl<'a> Binder<'a> {
    /// The cell whose privileges it binds with (`Context::who`).
    fn who(&self) -> Who<'a> {
        Who { auth: self.auth, role: self.role, roles: self.db.catalog.roles() }
    }
}

impl Binder<'_> {
    /// [X3.135-1986 §5.7] `<column specification>`: its position in the FROM row and data type.
    fn resolve(&self, cs: &ColumnSpecification) -> Result<(usize, DataType)> {
        self.resolve_column(cs).map(|c| (c.0, c.1)).or_at(cs.at)
    }

    /// [X3.135-1986 §5.7 SR3] Also reports whether the column is constrained
    /// NOT NULL, which a named result column inherits [X3.135-1986 §5.25 SR10].
    fn resolve_column(&self, cs: &ColumnSpecification) -> Result<(usize, DataType, bool)> {
        let (item, i, col) = match &cs.qualifier {
            Some(q) => self.qualified(q, &cs.column)?,
            None => self.unqualified(&cs.column)?,
        };
        Ok((self.note(item, i)?, col.data_type.clone(), col.not_null && !item.nullable))
    }

    /// Whether `c` is a column the FROM clause's results have.
    fn visible_has(&self, c: &str) -> bool {
        self.visible.iter().any(|&(i, k)| self.items.get(i).and_then(|it| it.desc.columns.get(k)).is_some_and(|col| col.name == c))
    }

    /// The position in the FROM row of column `i` of `item`, which the
    /// query uses: in a constraint, of a named table or view, one it holds
    /// REFERENCES on [ISO/IEC 9075:1992 §11.9 AR1a], a derived table's
    /// columns being of no `<table name>`, and its query's checked as they
    /// are bound. Noted among its uses, if it is of a named table or view,
    /// under the name it has there.
    fn note(&self, item: &FromItem, i: usize) -> Result<usize> {
        if self.purpose == Purpose::Constraint {
            if !item.table.auth.is_empty() {
                let needed = Actions { references: std::iter::once(i).collect(), ..Actions::default() };
                require(self.db, self.who(), &item.desc, &needed, "ISO/IEC 9075:1992 §11.9 AR1a")?;
            }
            if let Some(r) = self.referencing {
                r.column_named(&item.table);
            }
        }
        if let Some(uses) = self.uses.filter(|_| !item.table.auth.is_empty()) {
            let column = views::column_name(self.db, &item.table, i).or_else(|| item.desc.columns.get(i).map(|c| c.name.clone()));
            uses.borrow_mut().extend(column.map(|c| (item.table.clone(), c)));
        }
        // [ISO/IEC 9075-2:2023 §7.17 SR23b] a column of a query a <with
        // clause> names is drawn from what its query's column is.
        if let (Some(uses), Some(drawn)) = (self.uses, &item.drawn) {
            uses.borrow_mut().extend(drawn.columns.get(i).cloned().unwrap_or_default());
        }
        if let (Some(uses), views::Source::Common) = (self.uses, &item.source) {
            underlying::note_common(self, item, i, uses);
        }
        let pos = item.offset.saturating_add(item.position(i));
        self.used.borrow_mut().insert(pos);
        Ok(pos)
    }

    /// [X3.135-1986 §5.7 SR3a] column `c` of the table that `<qualifier>` `q` designates.
    fn qualified(&self, q: &TableName, c: &str) -> Result<(&FromItem, usize, &ColumnDesc)> {
        let name = q.auth.as_ref().map_or_else(|| q.ident.clone(), |a| format!("{a}.{}", q.ident));
        let it = self
            .items
            .iter()
            .find(|it| it.designated_by(q, self.schema))
            .ok_or_else(|| rule(format!("[X3.135-1986 §5.7 SR3a] no table or correlation name {name} is in scope")))?;
        let (i, col) =
            it.desc.column_desc(c).ok_or_else(|| no_column(&it.table, format!("[X3.135-1986 §5.7 SR3a] {name} has no column {c}"), c))?;
        if it.common.contains(&i) {
            return Err(rule(format!(
                "[ISO/IEC 9075:1992 §6.4 SR2b] {c} is a common column of a joined table {name} is in, so it is not qualified"
            )));
        }
        Ok((it, i, col))
    }

    /// [X3.135-1986 §5.7 SR3b] column `c` of the one table in scope that
    /// has it: of the columns the FROM clause's results have, which for a
    /// NATURAL or USING join are its common columns, not its operands'
    /// [ISO/IEC 9075:1992 §7.5 SR6f].
    fn unqualified(&self, c: &str) -> Result<(&FromItem, usize, &ColumnDesc)> {
        let possible: Vec<_> = self
            .visible
            .iter()
            .filter_map(|&(i, k)| {
                let it = self.items.get(i)?;
                it.desc.columns.get(k).filter(|col| col.name == c).map(|col| (it, k, col))
            })
            .collect();
        match possible.as_slice() {
            [only] => Ok(*only),
            [] => Err(rule(format!("[X3.135-1986 §5.7 SR3b] no table in the FROM clause has a column {c}"))),
            [_, _, ..] => Err(rule(format!("[X3.135-1986 §5.7 SR3b] column {c} is ambiguous; qualify it"))),
        }
    }
}
