//! \[EXTENSION\] [ISO/IEC 9075:1992 §17] Dynamic SQL in SQL text, run by a
//! session (RULES.md D57): its prepared statements, cursors and SQL
//! descriptor areas, named as §5.4 names them. The session plays the part
//! of the `<module>` [§5.4 GR12, GR13]: a `<statement name>` and a LOCAL
//! extended name are of it, a GLOBAL one of the SQL-session, and each is
//! the session's own, kept until it is deallocated or the session ends
//! [§20.1 GR2b], across transactions [§17.6 GR10].
//!
//! The statements take their values from the procedure's parameters
//! (`Params`), as any statement `Session::execute` runs does: so in direct
//! SQL, which has none [§20.1 SR1], only those whose operands may be
//! `<literal>`s are of use.
//!
//! Prepared statements are in `dynamic/statements.rs`, positioned
//! statements prepared in `dynamic/positioned.rs`, dynamic cursors in
//! `dynamic/cursors.rs`; SQL descriptor areas in `dynamic/descriptor.rs`
//! and their statements in `dynamic/areas.rs`; `<using clause>`s in
//! `dynamic/using.rs`.

use std::collections::HashMap;
use std::fmt;

use super::{Completion, Prepared, Session, StatementContext};
use crate::ast::{DynamicName, DynamicStatement, ExecuteStatement, ExtendedName, Scope, TableName, ValueSpecification};
use crate::binder;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::lexer::{Tok, Token, lex};
use crate::params::Params;
use crate::value::Value;

mod areas;
mod cursors;
mod descriptor;
mod positioned;
mod statements;
mod using;

pub(crate) use cursors::declared_statement;
pub(super) use descriptor::Descriptor;

/// [ISO/IEC 9075:1992 §5.4 SR14-SR16] The longest value of an extended
/// name or `<descriptor name>`, in octets.
const MAX_NAME_OCTETS: usize = 128;

/// [ISO/IEC 9075:1992 §5.4] The name of a prepared statement, a cursor or
/// an SQL descriptor area, in its scope: a `<statement name>` or `<cursor
/// name>`, of no `<scope option>`; or an extended name, the value of its
/// `<simple value specification>` without leading or trailing spaces.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Name {
    /// Its `<scope option>`, of an extended name.
    pub(crate) scope: Option<Scope>,
    /// The name.
    pub(crate) name: String,
}

impl Name {
    /// The `<cursor name>` or `<statement name>` `name`.
    pub(crate) fn plain(name: &str) -> Name {
        Name { scope: None, name: name.to_owned() }
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// A session's dynamic SQL: its prepared statements and SQL descriptor
/// areas; its cursors are kept with the others (`SessionState::cursors`).
#[derive(Debug, Default)]
pub(super) struct DynamicState {
    /// Its prepared statements [§17.6 GR7].
    statements: HashMap<Name, Prepared>,
    /// Its SQL descriptor areas [§17.2 GR2].
    descriptors: HashMap<Name, Descriptor>,
    /// The cursor the prepared statement running changes a row through,
    /// which its `<cursor name>` names [§17.6 GR2]: an extended cursor's
    /// too.
    pub(super) positioned: Option<Name>,
}

/// A dynamic SQL error (SQLCODE -921) of SQLSTATE `state`: `msg`.
pub(super) fn dynamic_error(state: SqlState, msg: String) -> SqlError {
    SqlError::new(Code::Dynamic, msg).with_state(state)
}

/// The value of `v`, a `<simple value specification>` that is a parameter
/// or a `<literal>`, and its type.
pub(super) fn value_of(session: &Session, v: &ValueSpecification, params: &Params) -> Result<(Value, crate::value::DataType)> {
    binder::value_specification(v, &session.auth(), params)
}

/// The value of `v`, exact with scale 0, as `rule` says it is, as an
/// `i64`: of at most 18 digits (`MAX_EXACT_PRECISION`), so it fits.
///
/// # Errors
///
/// SQLCODE -103 if it is not exact with scale 0.
pub(super) fn whole(session: &Session, v: &ValueSpecification, params: &Params, rule: &str) -> Result<i64> {
    match value_of(session, v, params)? {
        (Value::Exact(d), crate::value::DataType::Exact { scale: 0, .. }) => Ok(i64::try_from(d.units).unwrap_or(i64::MAX)),
        (_, _) => Err(SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075:1992 {rule}] the value is exact numeric with scale 0"))),
    }
}

impl Session {
    /// [ISO/IEC 9075:1992 §5.4 SR14-SR16] The value of extended name `n`,
    /// a character string of at most 128 octets, without leading or trailing
    /// spaces, which conforms to the Format and Syntax Rules of an
    /// `<identifier>`, else the exception `invalid` [§17.2 GR1, §17.6 GR8,
    /// §17.13 GR4].
    pub(super) fn extended(&self, n: &ExtendedName, params: &Params, invalid: (SqlState, &str)) -> Result<Name> {
        let (Value::Char(s), _) = value_of(self, &n.value, params)? else {
            let msg = "[ISO/IEC 9075:1992 §5.4 SR14-SR16] the value is a character string";
            return Err(SqlError::new(Code::SyntaxRule, msg).or_at(n.at));
        };
        if s.len() > MAX_NAME_OCTETS {
            let msg = format!("[ISO/IEC 9075:1992 §5.4 SR14-SR16] the name is longer than {MAX_NAME_OCTETS} octets");
            return Err(SqlError::new(Code::SyntaxRule, msg).or_at(n.at));
        }
        let name = s.trim_matches(' ');
        let conforms =
            lex(name).is_ok_and(|t| matches!(t.as_slice(), [Token { tok: Tok::Identifier(_), .. }, Token { tok: Tok::End, .. }]));
        if !conforms {
            let (state, rule) = invalid;
            return Err(dynamic_error(state, format!("[ISO/IEC 9075:1992 {rule}] '{name}' is not an <identifier>")).or_at(n.at));
        }
        Ok(Name { scope: Some(n.scope), name: name.to_owned() })
    }

    /// The name `n`, a `<statement name>` or `<cursor name>`, or an
    /// extended name as `extended` takes it.
    pub(super) fn dynamic_name(&self, n: &DynamicName, params: &Params, invalid: (SqlState, &str)) -> Result<Name> {
        match n {
            DynamicName::Named(name, _) => Ok(Name::plain(name)),
            DynamicName::Extended(e) => self.extended(e, params, invalid),
        }
    }

    /// [ISO/IEC 9075:1992 §17] Runs `d`, in the transaction `cx` gives: each
    /// is transaction initiating [§4.22.6], unlike EXECUTE and EXECUTE
    /// IMMEDIATE, which `execute_dynamic` runs.
    pub(super) fn run_dynamic(&self, cx: StatementContext<'_>, d: &DynamicStatement, params: &mut Params) -> Result<Completion> {
        match d {
            DynamicStatement::AllocateDescriptor { name, max } => self.allocate_descriptor(name, max.as_ref(), params),
            DynamicStatement::DeallocateDescriptor(name) => self.deallocate_descriptor(name, params),
            DynamicStatement::GetDescriptor { name, info } => self.get_descriptor(name, info, params),
            DynamicStatement::SetDescriptor { name, info } => self.set_descriptor(name, info, params),
            DynamicStatement::Prepare { name, source } => self.prepare_named(cx.st, name, source, params),
            DynamicStatement::DeallocatePrepare(name) => self.deallocate_prepare(name, params),
            DynamicStatement::Describe { input, name, descriptor } => self.describe(*input, name, descriptor, params),
            DynamicStatement::AllocateCursor { cursor, insensitive, scroll, statement } => {
                self.allocate_cursor(cursor, (*insensitive, *scroll), statement, params)
            }
            DynamicStatement::Open { cursor, using } => self.dynamic_open(cx, cursor, using.as_ref(), params),
            DynamicStatement::Fetch { cursor, orientation, using } => self.dynamic_fetch(cx, cursor, (orientation, using), params),
            DynamicStatement::Close(cursor) => self.dynamic_close(cursor, params),
            DynamicStatement::DeletePositioned { table, cursor } => self.dynamic_delete(cx, table, cursor, params),
            DynamicStatement::UpdatePositioned { table, set, cursor } => self.dynamic_update(cx, (table, set), cursor, params),
        }
    }
}

/// [ISO/IEC 9075:1992 §18.1 GR1c, Table 22] How COMMAND_FUNCTION names `d`.
pub(super) fn command(d: &DynamicStatement) -> &'static str {
    match d {
        DynamicStatement::AllocateDescriptor { .. } => "ALLOCATE DESCRIPTOR",
        DynamicStatement::DeallocateDescriptor(_) => "DEALLOCATE DESCRIPTOR",
        DynamicStatement::GetDescriptor { .. } => "GET DESCRIPTOR",
        DynamicStatement::SetDescriptor { .. } => "SET DESCRIPTOR",
        DynamicStatement::Prepare { .. } => "PREPARE",
        DynamicStatement::DeallocatePrepare(_) => "DEALLOCATE PREPARE",
        DynamicStatement::Describe { .. } => "DESCRIBE",
        DynamicStatement::AllocateCursor { .. } => "ALLOCATE CURSOR",
        DynamicStatement::Open { .. } => "DYNAMIC OPEN",
        DynamicStatement::Fetch { .. } => "DYNAMIC FETCH",
        DynamicStatement::Close(_) => "DYNAMIC CLOSE",
        DynamicStatement::DeletePositioned { .. } => "DYNAMIC DELETE CURSOR",
        DynamicStatement::UpdatePositioned { .. } => "DYNAMIC UPDATE CURSOR",
    }
}

/// The table `d` changes a row of, and the rule that refuses it in a READ
/// ONLY transaction [§13.6 GR1, §13.9 GR1, §17.17 GR1, §17.18 GR1]: of a
/// positioned DELETE or UPDATE through an extended cursor alone.
pub(super) fn changed_table(d: &DynamicStatement) -> Option<(&TableName, &'static str)> {
    match d {
        DynamicStatement::DeletePositioned { table, .. } => Some((table, "§13.6 GR1")),
        DynamicStatement::UpdatePositioned { table, .. } => Some((table, "§13.9 GR1")),
        DynamicStatement::AllocateDescriptor { .. }
        | DynamicStatement::DeallocateDescriptor(_)
        | DynamicStatement::GetDescriptor { .. }
        | DynamicStatement::SetDescriptor { .. }
        | DynamicStatement::Prepare { .. }
        | DynamicStatement::DeallocatePrepare(_)
        | DynamicStatement::Describe { .. }
        | DynamicStatement::AllocateCursor { .. }
        | DynamicStatement::Open { .. }
        | DynamicStatement::Fetch { .. }
        | DynamicStatement::Close(_) => None,
    }
}

/// [ISO/IEC 9075:1992 §18.1 GR1c, Table 22] How COMMAND_FUNCTION names `e`.
pub(super) fn execute_command(e: &ExecuteStatement) -> &'static str {
    match e {
        ExecuteStatement::Prepared { .. } => "EXECUTE",
        ExecuteStatement::Immediate(_) => "EXECUTE IMMEDIATE",
    }
}
