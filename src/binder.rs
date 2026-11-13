//! Binding: AST -> logical plan.
//!
//! The binder enforces the Syntax Rules that need the catalog: names exist
//! [X3.135-1986 §5.4 SR5], column references are unambiguous [X3.135-1986 §5.7 SR3], types are
//! comparable [X3.135-1986 §5.11 SR1], privileges are held [X3.135-1986 §5.25 SR1], grouped queries
//! reference only grouping columns outside set functions ([X3.135-1986 §5.25 SR7, §5.23 SR1]),
//! set operations' operands correspond [ISO/IEC 9075:1992 §7.10 SR9], and sort keys are valid
//! [X3.135-1986 §8.3 SR6].
//!
//! It then builds the plan exactly as the General Rules describe it, with no
//! optimization: the FROM product, then WHERE, GROUP BY, HAVING, the select
//! list, DISTINCT, the set operations, ORDER BY ([X3.135-1986 §5.19 GR1, §5.25, §8.3;
//! ISO/IEC 9075:1992 §7.10]). Making it fast is the optimizer's and the physical planner's job.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;

use crate::ast::{ColumnSpecification, SearchCondition, TableName, TruthValue, ValueExpression};
use crate::catalog::{Actions, ColumnDesc, Horizon, QualifiedName, TableDesc, WHOLE, applicable};
use crate::db::State;
use crate::error::{At, Code, Result, SqlError, Subject};
use crate::logical::{AggCall, Cond, Expr, LogicalPlan, Truth};
use crate::params::Params;
use crate::settings::{self, Own};
use crate::value::DataType;
use crate::value::collation::CollationRef;
use crate::value::datetime::Clocked;

mod arguments;
mod case;
mod collation;
mod composite;
pub(crate) use composite::grouping_operand;
mod cursor;
mod datetime;
mod interval;
pub(crate) use datetime::cast_is_valid;
pub use datetime::{function_type as datetime_function_type, literal_value as datetime_literal};
pub use interval::{displacement_of, is_displacement};
mod expression;
mod from;
mod generated;
mod greatest;
mod grouping_sets;
mod natural;
mod partitioned;
mod predicates;
mod purpose;
mod query;
mod recursive;
mod referencing;
mod routines;
mod search_cycle;
mod sequences;
mod set_functions;
mod set_operations;
mod statements;
mod strata;
mod subquery;
mod trim;
mod underlying;
mod unnest;
mod values;
mod views;
mod with;

pub use cursor::bind_cursor;
use cursor::sort_keys;
use from::FromItem;
pub use generated::{bind_generation, generations};
use query::bind_query_specification;
pub use set_operations::{ObjectColumn, bind_insert_query};
pub(crate) use statements::assignable;
pub use statements::{
    AssignRule, BoundWrite, INSERT_RULE, UPDATE_POSITIONED_RULE, UPDATE_SEARCHED_RULE, WriteKind, bind_select, bind_write,
    check_assignment, updatable, updatable_in,
};
use subquery::Outer;
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
    /// The schema an unqualified name is of [X3.135-1986 §5.4 SR2]: the
    /// session's default [ISO/IEC 9075:1992 §4.30, §16.2], `auth`'s unless
    /// SET SCHEMA set another; a view's owner's, within the view.
    pub schema: &'a str,
    /// [ISO/IEC 9075:1992 §4.26, §6.2 GR9] The current authorization
    /// identifier, the value of CURRENT_USER: the statement's, within a view
    /// it reads too.
    pub user: &'a str,
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
}

pub use purpose::Purpose;
pub use referencing::Referencing;

/// The columns a query references, each as its table or view and column
/// name, in the order referenced, once or more.
pub type Uses = RefCell<Vec<(QualifiedName, String)>>;

/// The columns each item of a select list references, as `Uses` notes
/// them, its subqueries' too.
pub type Selected = RefCell<Vec<Vec<(QualifiedName, String)>>>;

impl<'a> Context<'a> {
    /// A statement's query, run under `auth` with `params`.
    pub fn new(db: &'a State, auth: &'a str, params: &'a Params) -> Context<'a> {
        let (purpose, own) = (Purpose::Query, &settings::NONE);
        Context {
            now: db.now(own).into(),
            db,
            auth,
            schema: auth,
            user: auth,
            params,
            purpose,
            collation: None,
            horizon: &WHOLE,
            own,
            definitions: false,
            uses: None,
            selected: None,
            referencing: None,
            draws: false,
            queries: None,
        }
    }
}

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

/// [X3.135-1986 §6.6 SR3] Whether the applicable privileges of `auth` on
/// `table` include `needed`, as the Syntax Rule `clause` (e.g. "§8.7 SR1")
/// requires.
///
/// # Errors
///
/// SQLCODE -104 if they do not.
pub fn require(db: &State, auth: &str, table: &TableDesc, needed: &Actions, clause: &str) -> Result<()> {
    let (held, _) = applicable(&*db.catalog, auth, &table.name);
    let columns: Vec<String> = table.columns.iter().map(|c| c.name.clone()).collect();
    match held.missing(needed, &columns) {
        None => Ok(()),
        Some(missing) => {
            // [ISO/IEC 9075:1992 §18.1 GR3g] its table, and its column, if inaccessible.
            let column = held.missing_column(needed).and_then(|at| columns.get(at).cloned());
            let subject = Subject { column, ..Subject::table(table.name.parts()) };
            Err(SqlError::new(
                Code::Privilege,
                format!("[X3.135-1986 {clause}, §6.6 SR3] {auth} has no {missing} privilege on {}", table.name),
            )
            .about(subject))
        }
    }
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
// [X3.135-1986 §8.3] <cursor specification>
// ---------------------------------------------------------------------------

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
    /// [ISO/IEC 9075-2:2023 §7.13 SR17] Its grouping sets, of a GROUP BY
    /// not simple; none of a primitive one [SR10, SR11].
    sets: Option<grouping_sets::Sets>,
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
    /// The current authorization identifier (`Context::user`).
    user: &'a str,
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
    /// The query names in scope (`Context::queries`).
    queries: Option<&'a Queries<'a>>,
}

/// The FROM row a binder sees: its items, the display names of its
/// columns, and those seen unqualified (`From::visible_of`).
type Row = (Vec<FromItem>, Vec<String>, Vec<(usize, usize)>);

impl<'a> Binder<'a> {
    /// A binder of a query bound with `cx`, a subquery of `outer`'s if one
    /// is given, over the FROM row `row`, whose columns `used` it reads.
    fn new(cx: Context<'a>, outer: Option<&'a Outer<'a>>, (items, names, visible): Row, used: BTreeSet<usize>) -> Binder<'a> {
        let Context {
            db,
            auth,
            schema,
            user,
            params,
            purpose,
            collation,
            horizon,
            own,
            definitions,
            uses,
            selected,
            referencing,
            now,
            draws,
            queries,
        } = cx;
        let (used, aggregated) = (RefCell::new(used), Cell::new(false));
        Binder {
            db,
            params,
            auth,
            schema,
            items,
            names,
            used,
            outer,
            session_collation: collation,
            subquery_collations: RefCell::new(Vec::new()),
            purpose,
            visible,
            horizon,
            own,
            user,
            definitions,
            uses,
            selected,
            referencing,
            aggregated,
            now,
            draws: draws && !purpose.constrains(),
            drawing: Cell::new(false),
            queries,
        }
    }

    /// What its subqueries are bound with: what it is.
    fn context(&self) -> Context<'a> {
        let (db, auth, schema, user, params) = (self.db, self.auth, self.schema, self.user, self.params);
        let (purpose, horizon, own, definitions) = (self.purpose, self.horizon, self.own, self.definitions);
        let (referencing, now, collation) = (self.referencing, self.now, self.session_collation);
        Context {
            db,
            auth,
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
        }
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
                require(self.db, self.auth, &item.desc, &needed, "§11.9 AR1a")?;
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

    /// FROM-row column `pos`, of type `t`, in the rows `scope` sees. In a
    /// group it must be a grouping column [X3.135-1986 §5.25 SR7, §5.23
    /// SR1], its value of the group, which of grouping sets is null in a
    /// set that leaves it out [ISO/IEC 9075-2:2023 §7.13 SR17]; in a
    /// constraint, a grouping column not a character string one [ISO/IEC
    /// 9075:1992 §11.9 SR6, §7.8 SR3b, §7.9 SR13c ii].
    fn column_in(&self, pos: usize, t: &DataType, scope: Scope<'_>) -> Result<Expr> {
        let Scope::Groups(g, why) = scope else { return Ok(Expr::Column(pos)) };
        let found = match &g.sets {
            Some(sets) => sets.value_of(pos).cloned(),
            None => g.columns.iter().position(|&c| c == pos).map(Expr::Column),
        };
        let e = found.ok_or_else(|| rule(format!("{why} (found {})", self.names.get(pos).map_or("", String::as_str))))?;
        if self.purpose.constrains() && t.is_character() {
            return Err(nondeterministic("§7.8 SR3b, §7.9 SR13c ii", "a reference to a character string grouping column"));
        }
        Ok(e)
    }

    /// [X3.135-1986 §5.18] `<search condition>`
    fn search_condition(&self, c: &SearchCondition, scope: Scope<'_>) -> Result<Cond> {
        Ok(match c {
            SearchCondition::Or(a, b) => Cond::Or(Box::new(self.search_condition(a, scope)?), Box::new(self.search_condition(b, scope)?)),
            SearchCondition::And(a, b) => Cond::And(Box::new(self.search_condition(a, scope)?), Box::new(self.search_condition(b, scope)?)),
            SearchCondition::Not(a) => Cond::Not(Box::new(self.search_condition(a, scope)?)),
            // [ISO/IEC 9075:1992 §8.12] IS [NOT] TRUE, FALSE or UNKNOWN.
            SearchCondition::Is(a, value, not) => {
                let truth = match value {
                    TruthValue::True => Truth::True,
                    TruthValue::False => Truth::False,
                    TruthValue::Unknown => Truth::Unknown,
                };
                Cond::Is(Box::new(self.search_condition(a, scope)?), truth, *not)
            }
            SearchCondition::Predicate(p, at) => self.predicate(p, scope).or_at(*at)?,
            SearchCondition::Predicand(v, at) => self.predicand(v, scope).or_at(*at)?,
        })
    }

    /// [ISO/IEC 9075-2:2023 §6.46 SR3] A `<boolean predicand>` as a
    /// condition, a value of type BOOLEAN: a truth value written as one
    /// given back as its condition.
    fn predicand(&self, v: &ValueExpression, scope: Scope<'_>) -> Result<Cond> {
        let (e, t) = self.value_expression(v, scope)?;
        if t != DataType::Boolean {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.46 SR3] a <boolean predicand> is of type BOOLEAN (found {t})")));
        }
        let Expr::Truth(c) = e else { return Ok(Cond::Value(e)) };
        Ok(*c)
    }
}
