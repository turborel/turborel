//! [ISO/IEC 9075-4:2006 draft §14.2, §17.2, §17.3] Conditions in a
//! program: the most appropriate handler of one [§14.2 GR1], and the
//! conditions SIGNAL and RESIGNAL raise.
//!
//! A handler is associated with an SQLSTATE value, a class of them or a
//! condition declared for none [§14.2 SR10 to SR12]. The most appropriate
//! handler of a condition is of the innermost compound statement in scope
//! with one for it, its specific handler before its general one [SR8]. A
//! condition SIGNAL raises of a condition declared for no SQLSTATE value
//! has none: it is handled by its name alone, and no handler handling it,
//! it is the exception "unhandled user-defined exception", SQLSTATE 45000
//! [§17.3 GR6b iii].
//!
//! \[DESIGN\] No handler handles a condition that ends the transaction, or
//! the statement's cancellation: a serialization failure, a lost or
//! damaged write, or a cancel or timeout, after which no statement of the
//! transaction can go on [§14.2 GR3a].

use super::run::{Run, Scope, Stop};
use super::{Host, leaves};
use crate::ast::ConditionItem;
use crate::db::Completion;
use crate::error::{Code, Signalled, SqlError, SqlState, Subject};
use crate::logical::{Handled, Handler, Signal};
use crate::value::Value;

/// [§17.2 GR3] The items SIGNAL sets to a zero-length string, but for those
/// it gives a value.
const CLEARED: [ConditionItem; 11] = [
    ConditionItem::ClassOrigin,
    ConditionItem::SubclassOrigin,
    ConditionItem::ConstraintCatalog,
    ConditionItem::ConstraintSchema,
    ConditionItem::ConstraintName,
    ConditionItem::CatalogName,
    ConditionItem::SchemaName,
    ConditionItem::TableName,
    ConditionItem::ColumnName,
    ConditionItem::CursorName,
    ConditionItem::MessageText,
];

/// [§14.2 GR1] The most appropriate handler of `condition` among the
/// compound statements `scopes`, the innermost last, and the place of its
/// compound statement there; none if none handles it.
pub(super) fn appropriate<'p, M>(scopes: &[Scope<'p, M>], condition: &SqlError) -> Option<(usize, &'p Handler)> {
    let handlers: Vec<&'p [Handler]> = scopes.iter().map(|s| s.block.handlers.as_slice()).collect();
    let (k, h) = appropriate_among(&handlers, condition)?;
    handlers.get(k).and_then(|of| of.get(h)).map(|handler| (k, handler))
}

/// [§14.2 GR1] As `appropriate`, of the handlers of each compound
/// statement in scope, the innermost last: the place of the compound
/// statement and of its handler among its own.
pub(crate) fn appropriate_among(scopes: &[&[Handler]], condition: &SqlError) -> Option<(usize, usize)> {
    if !handleable(condition) {
        return None;
    }
    for (k, handlers) in scopes.iter().enumerate().rev() {
        let of =
            |specific: bool| handlers.iter().position(|h| h.conditions.iter().any(|c| c.specific() == specific && handles(c, condition)));
        if let Some(h) = of(true).or_else(|| of(false)) {
            return Some((k, h));
        }
    }
    None
}

/// Whether a handler may handle `e`: none handles one after which the
/// transaction cannot go on (the module's \[DESIGN\]).
fn handleable(e: &SqlError) -> bool {
    let ending =
        matches!(e.code, Code::Cancelled | Code::Conflict | Code::Damaged | Code::Io | Code::Corrupt | Code::InDoubt | Code::CommitFailed);
    !ending && e.sqlstate().class() != "40"
}

/// [§14.2 SR6, SR10 to SR12] Whether `handled` is associated with
/// `condition`'s SQLSTATE value, or with its condition name if it has
/// none.
fn handles(handled: &Handled, condition: &SqlError) -> bool {
    if let Some(name) = named(condition) {
        return *handled == Handled::Name(name.to_owned());
    }
    let state = condition.sqlstate();
    match handled {
        Handled::State(s) => *s == state,
        Handled::Name(_) => false,
        Handled::Warning => state.class() == "01",
        Handled::NotFound => state.class() == "02",
        Handled::Exception => !matches!(state.class(), "00" | "01" | "02"),
    }
}

/// The condition name of `e`, if SIGNAL raised it of a condition declared
/// for no SQLSTATE value [§17.2 GR2c].
fn named(e: &SqlError) -> Option<&str> {
    let s = e.subject.as_ref()?.signalled.as_ref()?;
    (s.item(ConditionItem::ReturnedSqlstate.name()) == Some("")).then_some(s.identifier.as_str())
}

/// The completion condition `completion`, as a handler is found for it.
pub(crate) fn completion(completion: Completion) -> SqlError {
    let state = completion.sqlstate();
    SqlError::new(Code::Signalled, format!("completion condition {}", state.as_str())).with_state(state)
}

/// [§17.3 GR6a iii] The completion condition SIGNAL or RESIGNAL raised as
/// `e`, of an SQLSTATE value of class 01 or 02; none of an exception.
pub(crate) fn completed(e: &SqlError) -> Option<Completion> {
    let state = e.state.filter(|_| e.code == Code::Signalled && named(e).is_none())?;
    match state.class() {
        "02" if state == SqlState::NO_DATA => Some(Completion::NoData),
        "01" | "02" => Some(Completion::Warning(state)),
        _ => None,
    }
}

/// [§17.3 GR6b iii] `e`, an exception no handler handled: of a condition
/// declared for no SQLSTATE value, "unhandled user-defined exception",
/// the SQLSTATE `signalled` gave it, its zero-length RETURNED_SQLSTATE
/// dropped.
pub(crate) fn unhandled(mut e: SqlError) -> SqlError {
    if named(&e).is_some()
        && let Some(s) = e.subject.as_mut().and_then(|s| s.signalled.as_mut())
    {
        s.items.retain(|(n, _)| *n != ConditionItem::ReturnedSqlstate.name());
    }
    e
}

impl<H: Host> Run<'_, '_, H> {
    /// [§17.2, §17.3] The condition SIGNAL or RESIGNAL `s` raises: of
    /// RESIGNAL of no `<signal value>`, the condition the innermost handler
    /// executing handles, its items set as `s` sets them.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0K000 of RESIGNAL where no handler is executing [§17.3
    /// GR1]; an item's value's.
    pub(super) fn signal(&mut self, s: &Signal) -> Result<SqlError, Stop> {
        let mut items = Vec::new();
        for (item, leaf) in &s.items {
            let value = leaves::value(self.host, leaf, self.frame).map_err(Stop::Raised)?;
            items.push((item.name(), item_text(value)));
        }
        raised(s, items, self.handling.last()).map_err(Stop::Raised)
    }
}

/// [§17.2, §17.3] The condition SIGNAL or RESIGNAL `s` raises, its items
/// set `items`: of RESIGNAL of no `<signal value>`, the condition
/// `handling`, the one the innermost handler executing handles, with them.
///
/// # Errors
///
/// SQLSTATE 0K000 of RESIGNAL where no handler is executing [§17.3 GR1].
pub(crate) fn raised(s: &Signal, items: Vec<(&'static str, String)>, handling: Option<&SqlError>) -> Result<SqlError, SqlError> {
    if !s.resignal {
        return Ok(signalled(s, items, false));
    }
    let Some(handled) = handling else {
        let msg = "[ISO/IEC 9075-4:2006 draft §17.3 GR1] RESIGNAL where no handler is executing";
        return Err(SqlError::new(Code::Data, msg).with_state(SqlState::RESIGNAL_NOT_ACTIVE).or_at(s.at));
    };
    if s.state.is_some() || s.name.is_some() {
        return Ok(signalled(s, items, true));
    }
    Ok(resignalled(handled.clone(), items))
}

/// [§17.2 GR3] An item's value as SIGNAL sets it: of a character string,
/// itself; of the null value, a zero-length string; else as shown.
pub(crate) fn item_text(value: Value) -> String {
    if let Value::Char(text) = value {
        text.to_string()
    } else if value.is_null() {
        String::new()
    } else {
        value.to_string()
    }
}

/// [§17.2 GR1 to GR3] The condition SIGNAL `s` raises, of the items set
/// `items`: of its SQLSTATE value, else of none, the items it gives none
/// zero-length strings; `stacked` of a RESIGNAL, raised ahead of the
/// condition its handler handles [§17.3 GR5].
fn signalled(s: &Signal, items: Vec<(&'static str, String)>, stacked: bool) -> SqlError {
    let mut set: Vec<(&'static str, String)> = CLEARED.iter().map(|i| (i.name(), String::new())).collect();
    for (name, value) in items {
        if let Some(slot) = set.iter_mut().find(|(n, _)| *n == name) {
            slot.1 = value;
        }
    }
    if s.state.is_none() {
        set.push((ConditionItem::ReturnedSqlstate.name(), String::new()));
    }
    let state = s.state.unwrap_or(SqlState::UNHANDLED_USER_EXCEPTION);
    let identifier = s.name.clone().unwrap_or_default();
    let message = set.iter().find(|(n, _)| *n == ConditionItem::MessageText.name()).map(|(_, m)| m.clone()).unwrap_or_default();
    let message = if message.is_empty() { described(state, &identifier) } else { message };
    let subject = Subject { signalled: Some(Box::new(Signalled { identifier, items: set, stacked })), ..Subject::default() };
    SqlError::new(Code::Signalled, message).with_state(state).about(subject).or_at(s.at)
}

/// [§17.3 GR4] The condition `handled` resignalled, `items` set of it.
fn resignalled(mut handled: SqlError, items: Vec<(&'static str, String)>) -> SqlError {
    if let Some((_, text)) = items.iter().find(|(n, _)| *n == ConditionItem::MessageText.name()) {
        handled.msg.clone_from(text);
        handled.rule = None;
    }
    let subject = handled.subject.get_or_insert_with(Box::default);
    let signalled = subject.signalled.get_or_insert_with(Box::default);
    for (name, value) in items {
        signalled.items.retain(|(n, _)| *n != name);
        signalled.items.push((name, value));
    }
    handled
}

/// The message of a condition signalled with no MESSAGE_TEXT.
fn described(state: SqlState, identifier: &str) -> String {
    if identifier.is_empty() {
        format!("[ISO/IEC 9075-4:2006 draft §17.2] SQLSTATE '{}' signalled", state.as_str())
    } else {
        format!("[ISO/IEC 9075-4:2006 draft §17.2] condition {identifier} signalled")
    }
}
