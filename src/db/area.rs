//! \[EXTENSION\] A session's diagnostics area [ISO/IEC 9075:1992 §4.19],
//! and GET DIAGNOSTICS, which reads it [§18.1; ISO/IEC 9075-2:2023 §23.1].
//!
//! Every statement but GET DIAGNOSTICS empties the area as it begins, and
//! places its condition there as it ends: the one its SQLCODE and SQLSTATE
//! report, successful completion too [§4.18], so the area holds one
//! condition after each statement, none before the session's first.
//! Another it raised is left out, and MORE says so. What the condition is
//! about, a constraint, table, column or cursor, is what its error names
//! (`error::Subject`). The transactions the statement committed and
//! rolled back are counted there too (Feature T511).
//!
//! A warning an expression raises as the statement runs, of a CAST
//! [§6.10] or a set function [§6.5 GR1b], is heard by the session's
//! `error::Warnings`, emptied as the area is, and is the statement's
//! completion if it would be successful (`Completion::warned`).

pub(super) mod commands;

mod all;
mod read;
mod stack;

use self::commands::Command;
pub(super) use self::read::Reader;
use super::assignment::target_type;
use super::roles::Valued;
use super::{Completion, Session, State, rule};
use crate::ast::{ConditionItem, GetDiagnostics, ParameterSpecification, StatementItem, ValueSpecification};
use crate::binder::assignable;
use crate::error::{Code, Result, RoutineSubject, Signalled, SqlState, Subject, fail_with};
use crate::lexer::kw;
use crate::params::Params;
use crate::value::{DataType, Decimal, MAX_CHARACTER_LENGTH, MAX_EXACT_PRECISION, Spelled, Value};

/// \[DESIGN\] The naming authority of the conditions this implementation
/// defines, its SQLSTATE class TB [§18.1 GR3b, GR3c].
const IMPLEMENTATION: &str = "TURBOREL";
/// [ISO/IEC 9075:1992 §18.1 GR3f ii, GR3g i] The CATALOG_NAME of a
/// declared local temporary table: <space>s, here one.
const DECLARED_CATALOG: &str = " ";
/// [ISO/IEC 9075:1992 §18.1 GR3b, GR3c] The naming authority of the
/// conditions the standard defines.
const STANDARD: &str = "ISO 9075";
/// [ISO/IEC 9075:1992 §22.1, Table 23] The class of a warning.
const WARNING_CLASS: &str = "01";
/// [ISO/IEC 9075:1992 §4.30, §12.3 GR6c i, §18.1 GR3n ii] The connection a
/// session is on, and its server, by the word that stands for them in the
/// CONNECT TO DEFAULT it effectively ran: only a host program reads this
/// area, its session in the process, on no named connection.
const DEFAULT_CONNECTION: &str = kw::DEFAULT;

/// \[DESIGN\] [ISO/IEC 9075:1992 §4.19] What the last statement but GET
/// DIAGNOSTICS raised.
#[derive(Debug, Clone, Default)]
pub(super) struct Area {
    /// [§18.1 GR1c, Table 22] The statement, as COMMAND_FUNCTION and
    /// [ISO/IEC 9075-2:2023 §23.1 GR2d] COMMAND_FUNCTION_CODE name it.
    command: Command,
    /// [§18.1 GR1d] The prepared statement it executed, as
    /// DYNAMIC_FUNCTION and DYNAMIC_FUNCTION_CODE name it; unrecognized
    /// for one not prepared.
    dynamic: Command,
    /// [§18.1 GR1e] The rows it inserted, updated or deleted; 0 for a
    /// statement of another kind.
    row_count: usize,
    /// [ISO/IEC 9075-2:2023 §23.1 GR2h, GR2i] The transactions it
    /// committed, and those it rolled back.
    ended: Ended,
    /// Its condition, once it ended.
    condition: Option<Condition>,
    /// The warning taken as it ended, kept to see if its condition is
    /// another.
    heard: Option<SqlState>,
    /// [§18.1 GR1b] Whether it raised a condition not stored.
    more: bool,
    /// [ISO/IEC 9075-2:2023 §23.2] The areas below it on the diagnostics
    /// area stack, pushed as handlers were activated, the second last: the
    /// one GET STACKED DIAGNOSTICS reads (`area/stack.rs`).
    stacked: Vec<Area>,
}

/// [ISO/IEC 9075-2:2023 §23.1 GR2h, GR2i] The SQL-transactions a
/// statement ended.
#[derive(Debug, Clone, Copy, Default)]
struct Ended {
    committed: usize,
    rolled_back: usize,
}

/// A condition placed in the area.
#[derive(Debug, Clone)]
struct Condition {
    state: SqlState,
    /// MESSAGE_TEXT [§18.1 GR3k]: an error's message; empty for a
    /// completion.
    message: String,
    subject: Subject,
}

impl Condition {
    /// [ISO/IEC 9075-4:2006 draft §17.3 GR5b] Whether a RESIGNAL of a
    /// `<signal value>` raised it ahead of a condition, which the area,
    /// of one condition, leaves out.
    fn stacked(&self) -> bool {
        self.subject.signalled.as_ref().is_some_and(|s| s.stacked)
    }
}

impl Session {
    /// [ISO/IEC 9075:1992 §4.19] The area emptied, as a statement other
    /// than GET DIAGNOSTICS begins: one `command` names, of the prepared
    /// statement `dynamic` names, if one.
    pub(super) fn emptied(&self, (command, dynamic): (Command, Command)) {
        self.inner.borrow_mut().area.begin((command, dynamic));
        self.stop.warnings().take();
        self.stop.warnings().take_left_out();
    }

    /// [ISO/IEC 9075-2:2023 §23.1 GR2h, GR2i] The session's transaction
    /// ended, committed or not, counted in the area.
    pub(super) fn transaction_ended(&self, committed: bool) {
        let ended = &mut self.inner.borrow_mut().area.ended;
        let count = if committed { &mut ended.committed } else { &mut ended.rolled_back };
        *count = count.saturating_add(1);
    }

    /// [ISO/IEC 9075-2:2023 §23.1 GR2h, GR2i] A commit whose outcome is
    /// `ended` counted: committed if it succeeded, or if it had
    /// `committed` before a failure after; else rolled back [X3.135-1986
    /// §8.2], unless it is held in doubt (RULES.md D25), counted neither.
    /// Whether it is in doubt, and so to be held, not ended.
    pub(super) fn commit_ended(&self, ended: &Result<()>, committed: bool) -> bool {
        let in_doubt = ended.as_ref().err().is_some_and(|e| e.code == Code::InDoubt);
        if !in_doubt {
            self.transaction_ended(ended.is_ok() || committed);
        }
        in_doubt
    }

    /// [ISO/IEC 9075:1992 §3.3.4.1] The warning the statement raised as it
    /// ran, if it raised one: the first, taken.
    pub(super) fn raised(&self) -> Option<SqlState> {
        let raised = self.stop.warnings().take();
        self.inner.borrow_mut().area.heard = raised;
        raised
    }

    /// \[EXTENSION\] [ISO/IEC 9075:1992 §4.19] The warning the last
    /// statement but GET DIAGNOSTICS completed with, if it did, as the
    /// diagnostics area holds it: of a query whose rows a `Cursor` reads,
    /// the warning its rows raised as it was opened, as they were read then
    /// if they could raise one (RULES.md D15).
    pub fn warning(&self) -> Option<SqlState> {
        self.inner.borrow().area.condition.as_ref().map(|c| c.state).filter(|state| state.class() == WARNING_CLASS)
    }

    /// The rows the last statement changed, as ROW_COUNT tells.
    pub(in crate::db) fn rows_affected(&self) -> usize {
        self.inner.borrow().area.row_count
    }

    /// [ISO/IEC 9075:1992 §18.1 GR1e] The statement changed `rows` rows:
    /// its completion, no data if none [X3.135-1986 §7.3 GR3a iv].
    pub(super) fn affected(&self, rows: usize) -> Completion {
        self.inner.borrow_mut().area.row_count = rows;
        if rows == 0 { Completion::NoData } else { Completion::Success }
    }

    /// [ISO/IEC 9075:1992 §4.19] The condition of a statement's `outcome`
    /// placed in the area, a completion's SQLSTATE `state` gives: `outcome`.
    pub(super) fn diagnosed<T>(&self, outcome: Result<T>, state: impl FnOnce(&T) -> SqlState) -> Result<T> {
        let condition = match &outcome {
            Ok(done) => Condition { state: state(done), message: String::new(), subject: Subject::default() },
            Err(e) => Condition { state: e.sqlstate(), message: e.described(), subject: e.subject.as_deref().cloned().unwrap_or_default() },
        };
        let warnings = self.stop.warnings();
        let unheard = warnings.take();
        let left_out = warnings.take_left_out();
        let area = &mut self.inner.borrow_mut().area;
        // [§18.1 GR1b] an exception, or another warning, in its place.
        let replaced = area.heard.take().or(unheard).is_some_and(|w| w != condition.state);
        // [ISO/IEC 9075-2:2023 §15.21 GR6m] a triggered action exception
        // is raised with the condition that caused it, which is left out.
        let caused = condition.state == SqlState::TRIGGERED_ACTION_EXCEPTION;
        area.more |= left_out || replaced || caused || condition.stacked();
        area.condition = Some(condition);
        outcome
    }

    /// The condition in the area about cursor `name` [ISO/IEC 9075:1992
    /// §18.1 GR3e].
    pub(super) fn about_cursor(&self, name: &str) {
        if let Some(condition) = self.inner.borrow_mut().area.condition.as_mut() {
            condition.subject.cursor = Some(name.to_owned());
        }
    }

    /// [ISO/IEC 9075:1992 §18.1; ISO/IEC 9075-2:2023 §23.1] GET
    /// DIAGNOSTICS `get`: what the area holds assigned to its targets; the
    /// area as it was.
    ///
    /// # Errors
    ///
    /// As `Reader::get`'s; SQLSTATE 0Z002 of STACKED where no handler is
    /// active.
    pub(super) fn get_diagnostics(&self, st: &State, get: &GetDiagnostics, params: &mut Params) -> Result<Completion> {
        let area = self.read_area(get.stacked)?;
        // [§23.1 GR2j] whether one is active now, as GET DIAGNOSTICS begins
        // none [ISO/IEC 9075:1992 §4.28].
        let active = self.inner.borrow().tx.is_some();
        let authorization = self.authorization();
        Reader { st, user: authorization.of(st.catalog.roles()), active }.get(&area, get, params)
    }
}

impl Area {
    /// [ISO/IEC 9075:1992 §4.19] The area emptied, as a statement other than
    /// GET DIAGNOSTICS begins: one `command` names, of the prepared
    /// statement `dynamic` names, if one; the stack below it kept.
    pub(in crate::db) fn begin(&mut self, (command, dynamic): (Command, Command)) {
        *self = Area { command, dynamic, stacked: std::mem::take(&mut self.stacked), ..Area::default() };
    }

    /// [ISO/IEC 9075:1992 §4.19] The condition of a statement's `outcome`
    /// placed in the area, of a statement no warning is heard of apart.
    pub(in crate::db) fn place(&mut self, outcome: &Result<Completion>) {
        let condition = match outcome {
            Ok(done) => Condition { state: done.sqlstate(), message: String::new(), subject: Subject::default() },
            Err(e) => Condition { state: e.sqlstate(), message: e.described(), subject: e.subject.as_deref().cloned().unwrap_or_default() },
        };
        self.more |= condition.stacked();
        self.condition = Some(condition);
    }

    /// The value of statement information item `item`, read while a
    /// transaction is `active` or not [ISO/IEC 9075:1992 §18.1 GR1;
    /// ISO/IEC 9075-2:2023 §23.1 GR2].
    fn value(&self, item: StatementItem, active: bool) -> Value {
        match item {
            StatementItem::Number => number(self.conditions().count()),
            StatementItem::More => text(if self.more { "Y" } else { "N" }),
            StatementItem::CommandFunction => text(self.command.identifier),
            StatementItem::CommandFunctionCode => code(self.command),
            StatementItem::DynamicFunction => text(self.dynamic.identifier),
            StatementItem::DynamicFunctionCode => code(self.dynamic),
            StatementItem::RowCount => number(self.row_count),
            StatementItem::TransactionsCommitted => number(self.ended.committed),
            StatementItem::TransactionsRolledBack => number(self.ended.rolled_back),
            StatementItem::TransactionActive => number(usize::from(active)),
        }
    }

    /// Each condition the area holds, by its number: the one its statement
    /// raised, if it has ended.
    fn conditions(&self) -> impl Iterator<Item = (i128, &Condition)> {
        self.condition.iter().map(|c| (1, c))
    }

    /// [ISO/IEC 9075-2:2023 §23.1 SR3, GR3] The condition the `<condition
    /// number>` `n` names, and its number.
    fn numbered(&self, n: &ValueSpecification, valued: &Valued<'_>) -> Result<(i128, &Condition)> {
        let n = condition_number(n, valued)?;
        if let Some((_, condition)) = self.conditions().find(|(at, _)| *at == n) {
            return Ok((n, condition));
        }
        let msg = format!("[ISO/IEC 9075:1992 §18.1 GR2] there is no condition {n}: the area holds {}", self.conditions().count());
        fail_with(Code::Data, SqlState::INVALID_CONDITION_NUMBER, msg)
    }
}

/// What a condition names, as GET DIAGNOSTICS gives it.
struct Shown {
    catalog: String,
    constraint: Option<(String, String)>,
    table: Option<(String, String)>,
    column: Option<String>,
    cursor: Option<String>,
    trigger: Option<(String, String)>,
    routine: Option<RoutineSubject>,
    signalled: Option<Box<Signalled>>,
}

impl Shown {
    /// [ISO/IEC 9075:1992 §18.1 GR3] The value of `item` for condition
    /// `n`, `condition`: a zero-length string for a name it has not
    /// \[GR3o\]; [ISO/IEC 9075-4:2006 draft §17.2 GR4] of a condition SIGNAL
    /// or RESIGNAL raised, the value it set the item to, if it set it, the
    /// lengths those of the message it set.
    fn value(&self, item: ConditionItem, (n, condition): (i128, &Condition)) -> Value {
        let set = |name: &str| self.signalled.as_ref().and_then(|s| s.item(name));
        if let Some(v) = set(item.name()) {
            return text(v);
        }
        let message = set(ConditionItem::MessageText.name()).unwrap_or(&condition.message);
        // [GR3f ii, GR3g i] <space>s, one, for a declared local temporary
        // table's, in no catalog.
        let catalog_of = |named: &Option<(String, String)>| match named {
            Some((schema, _)) => text(if schema == kw::MODULE { DECLARED_CATALOG } else { &self.catalog }),
            None => text(""),
        };
        let schema_of = |named: &Option<(String, String)>| text(named.as_ref().map_or("", |(s, _)| s));
        let name_of = |named: &Option<(String, String)>| text(named.as_ref().map_or("", |(_, i)| i));
        let origin = if condition.state.class() == "TB" { IMPLEMENTATION } else { STANDARD };
        match item {
            ConditionItem::ConditionNumber => Value::Exact(Decimal::new(n, 0)),
            ConditionItem::ReturnedSqlstate => text(condition.state.as_str()),
            ConditionItem::ClassOrigin | ConditionItem::SubclassOrigin => text(origin),
            ConditionItem::ServerName | ConditionItem::ConnectionName => text(DEFAULT_CONNECTION),
            ConditionItem::ConstraintCatalog => catalog_of(&self.constraint),
            ConditionItem::ConstraintSchema => schema_of(&self.constraint),
            ConditionItem::ConstraintName => name_of(&self.constraint),
            ConditionItem::CatalogName => catalog_of(&self.table),
            ConditionItem::SchemaName => schema_of(&self.table),
            ConditionItem::TableName => name_of(&self.table),
            ConditionItem::ColumnName => text(self.column.as_deref().unwrap_or("")),
            ConditionItem::CursorName => text(self.cursor.as_deref().unwrap_or("")),
            ConditionItem::MessageText => text(message),
            ConditionItem::MessageLength => number(message.chars().count()),
            ConditionItem::MessageOctetLength => number(message.len()),
            // [ISO/IEC 9075-2:2023 §23.1 GR4g] the trigger of a triggered
            // action exception.
            ConditionItem::TriggerCatalog => catalog_of(&self.trigger),
            ConditionItem::TriggerSchema => schema_of(&self.trigger),
            ConditionItem::TriggerName => name_of(&self.trigger),
            // [ISO/IEC 9075-2:2023 §23.1 GR4l, GR4q, GR5] of an SQL-invoked
            // routine's condition; else 0 and zero-length strings. A
            // condition of a distinct type's function is one of the CAST it
            // stands for.
            ConditionItem::RoutineCatalog => catalog_of(&self.routine.as_ref().map(|r| r.name.clone())),
            ConditionItem::RoutineSchema => schema_of(&self.routine.as_ref().map(|r| r.name.clone())),
            ConditionItem::RoutineName => name_of(&self.routine.as_ref().map(|r| r.name.clone())),
            ConditionItem::SpecificName => text(self.routine.as_ref().map_or("", |r| &r.specific)),
            ConditionItem::ParameterOrdinalPosition => number(self.parameter().map_or(0, |p| p.0)),
            ConditionItem::ParameterMode => text(self.parameter().map_or("", |p| p.1)),
            ConditionItem::ParameterName => text(self.parameter().map_or("", |p| &p.2)),
            // [ISO/IEC 9075-4:2006 draft §17.2 GR2b] the condition name SIGNAL
            // or RESIGNAL raised the condition of.
            ConditionItem::ConditionIdentifier => text(self.signalled.as_ref().map_or("", |s| &s.identifier)),
        }
    }
}

impl Shown {
    /// The parameter its routine's condition is related to, if any.
    fn parameter(&self) -> Option<&(usize, &'static str, String)> {
        self.routine.as_ref()?.parameter.as_ref()
    }
}

fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

fn number(n: impl TryInto<i128>) -> Value {
    Value::Exact(Decimal::new(n.try_into().unwrap_or(i128::MAX), 0))
}

/// [ISO/IEC 9075-2:2023 §23.1 GR2d, GR2e] The code of `command`.
fn code(command: Command) -> Value {
    Value::Exact(Decimal::new(i128::from(command.code), 0))
}

/// [ISO/IEC 9075-2:2023 §23.1 Table 37, Table 38] An item's declared type:
/// exact numeric with scale 0 if `numeric`, here of the greatest
/// precision, else a character string, here of the longest a varying one
/// may be (IL205).
fn declared(numeric: bool) -> DataType {
    if numeric {
        DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric }
    } else {
        DataType::varying(MAX_CHARACTER_LENGTH)
    }
}

/// [ISO/IEC 9075-2:2023 §23.1 SR1, §9.1 SR3, SR5b] The targets and their
/// values, each item's type assignable to its target's (Feature F122):
/// an exact numeric item to a target of any numeric type, a character
/// string item to one of any character string type.
fn typed<'a>(
    items: impl Iterator<Item = (&'a ParameterSpecification, bool, Value)>,
    params: &Params,
) -> Result<(Vec<ParameterSpecification>, Vec<Value>)> {
    let (mut targets, mut values) = (Vec::new(), Vec::new());
    for (target, numeric, value) in items {
        let target_type = target_type(params, target)?;
        let item_type = declared(numeric);
        if !assignable(&target_type, &item_type) {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §23.1 SR1, §9.1 SR3] a {item_type} item cannot be assigned to {} ({target_type})",
                target.name
            );
            return Err(rule(msg).or_at(target.at));
        }
        targets.push(target.clone());
        values.push(value);
    }
    Ok((targets, values))
}

/// [ISO/IEC 9075-2:2023 §23.1 SR2] The `<all info target>` `target`, of a
/// character string type.
fn all_target(target: &ParameterSpecification, params: &Params) -> Result<ParameterSpecification> {
    let target_type = target_type(params, target)?;
    if !target_type.is_character() {
        let msg = format!("[ISO/IEC 9075-2:2023 §23.1 SR2] target {} of ALL is {target_type}, not a character string", target.name);
        return Err(rule(msg).or_at(target.at));
    }
    Ok(target.clone())
}

/// [ISO/IEC 9075-2:2023 §23.1 SR3] The `<condition number>` `n`'s value.
fn condition_number(n: &ValueSpecification, valued: &Valued<'_>) -> Result<i128> {
    let not_exact = || rule("[ISO/IEC 9075-2:2023 §23.1 SR3] the <condition number> is exact numeric with scale 0".to_owned());
    let (value, data_type) = valued(n)?;
    if !matches!(data_type, DataType::Exact { scale: 0, .. }) {
        return Err(not_exact());
    }
    match value {
        Value::Exact(d) => Ok(d.units),
        Value::Null => fail_with(
            Code::Data,
            SqlState::INVALID_CONDITION_NUMBER,
            "[ISO/IEC 9075:1992 §18.1 GR2] the <condition number> is null".to_owned(),
        ),
        Value::Char(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Lob(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Json(_)
        | Value::Extension(_) => Err(not_exact()),
    }
}
