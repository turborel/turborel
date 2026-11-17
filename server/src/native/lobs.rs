//! \[DESIGN\] Large objects over the protocol (RULES.md D45, D79): a
//! result's large object sent whole if it is at most SERVER.LOB_INLINE_BYTES
//! long, else by a locator of the session's [ISO/IEC 9075-2:2023 §4.37.5],
//! which the client reads a chunk at a time by Lob requests; and those
//! requests run. A locator is valid until its transaction ends, so a
//! request that would end the transaction with its statement keeps it
//! once a locator is sent, as a cursor left open does.

use turborel::db::Session;
use turborel::error::Result;
use turborel::storage::Row;
use turborel::value::Value;
use turborel::value::lob::{Lob, LobKind};
use turborel_protocol::{LobReply, LobRequest, Reply};

use super::requests::Held;
use turborel::db::Completion;

/// `row`, each large object in it held whole if at most `inline` octets
/// long, read now, else by a new locator of `session`'s; `located` set
/// if one is.
///
/// # Errors
///
/// Reading a large object's.
pub(super) fn placed_row(session: &Session, row: Row, inline: usize, located: &mut bool) -> Result<Row> {
    if !row.iter().any(holds_lob) {
        return Ok(row);
    }
    row.into_iter().map(|v| placed(session, v, inline, located)).collect()
}

/// Whether `v` is or holds a large object.
fn holds_lob(v: &Value) -> bool {
    match v {
        Value::Lob(_) => true,
        Value::Array(items) | Value::Multiset(items) => items.iter().any(holds_lob),
        Value::Row(fields) => fields.iter().any(holds_lob),
        Value::Structured(s) => s.attributes.iter().any(holds_lob),
        Value::Null
        | Value::Reference(_)
        | Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Extension(_) => false,
    }
}

/// `v`, as `placed_row` places a row's values.
fn placed(session: &Session, v: Value, inline: usize, located: &mut bool) -> Result<Value> {
    match v {
        Value::Lob(l) => placed_lob(session, l, inline, located).map(Value::Lob),
        Value::Array(items) => {
            Ok(Value::Array(items.iter().cloned().map(|i| placed(session, i, inline, located)).collect::<Result<Vec<_>>>()?.into()))
        }
        Value::Multiset(items) => {
            Ok(Value::Multiset(items.iter().cloned().map(|i| placed(session, i, inline, located)).collect::<Result<Vec<_>>>()?.into()))
        }
        Value::Row(fields) => {
            Ok(Value::Row(fields.iter().cloned().map(|f| placed(session, f, inline, located)).collect::<Result<Vec<_>>>()?.into()))
        }
        Value::Structured(s) => {
            let attributes = s.attributes.iter().cloned().map(|a| placed(session, a, inline, located)).collect::<Result<Vec<_>>>()?;
            Ok(Value::Structured(std::sync::Arc::new(turborel::value::types::Structured::new(
                std::sync::Arc::clone(&s.of),
                attributes.into(),
            ))))
        }
        Value::Null
        | Value::Reference(_)
        | Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Extension(_) => Ok(v),
    }
}

/// `l` held whole if at most `inline` octets long, else a locator of it.
fn placed_lob(session: &Session, l: Lob, inline: usize, located: &mut bool) -> Result<Lob> {
    if l.inline().is_some() {
        return Ok(l);
    }
    if l.octets() <= u64::try_from(inline).unwrap_or(u64::MAX) {
        let bytes = l.reader().read(0, l.octets())?;
        return Ok(match l.kind() {
            LobKind::Character => Lob::of_text(&String::from_utf8_lossy(&bytes)),
            LobKind::Binary => Lob::of_octets(&bytes),
        });
    }
    let (kind, totals) = (l.kind(), (l.octets(), l.chars()));
    *located = true;
    Ok(Lob::of_locator(kind, session.locator_of(l), totals))
}

impl Held {
    /// Runs `request`: the reply that gives back what it asked for, or its
    /// completion.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0F001 if its locator is not valid; the session's.
    pub(super) fn lob(&mut self, request: LobRequest) -> Result<Reply> {
        let done = || Reply::Complete(Completion::Success.into());
        match request {
            LobRequest::Read { locator, from, count, characters } => {
                self.session.read_lob(locator, (from, count), characters).map(|bytes| Reply::Lob(LobReply::Data(bytes)))
            }
            LobRequest::Create(kind) => Ok(Reply::Lob(LobReply::Locator(self.session.create_lob(kind)))),
            LobRequest::Append { locator, bytes } => self.session.append_lob(locator, &bytes).map(|()| done()),
            LobRequest::Free(locator) => self.session.free_locator(locator).map(|()| done()),
            LobRequest::Hold(locator) => self.session.hold_locator(locator).map(|()| done()),
        }
    }
}
