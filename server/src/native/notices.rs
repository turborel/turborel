//! \[DESIGN\] Notices (RULES.md D187): sent to a client granted `notices`
//! before the completion of the statement that raised them, in the order
//! raised. A statement raises one for each SIGNAL of class 01 or 02 of its
//! routines no handler handled, with the message it set, and one for the
//! completion condition of class 01 it completed with, unless a SIGNAL
//! raised that one, named as [ISO/IEC 9075:1992 §22.1 Table 23] names its
//! subclass. The server raises its own, of class 01 too, as a Reset or a
//! ChangeUser rolls back the transaction it found active.
//!
//! A statement's conditions are one list, told one way: in its Complete,
//! each as an Error's body, to a client granted `conditions` (D187, for
//! issue 10's drivers' warnings); else as a Notice each, before its last
//! reply, to one granted `notices`; else not at all. The server's own,
//! of no statement, are always a Notice.

use turborel::db::Session;
use turborel::error::SqlState;
use turborel_protocol::{Diagnostics, Failure, NOTICE, Notice, Reply, WARNING};

/// [ISO/IEC 9075:1992 §22.1 Table 23] The class of a warning.
const WARNING_CLASS: &str = "01";
/// [ISO/IEC 9075:1992 §22.1 Table 23] The class of no data, and its
/// SQLCODE [Table 24].
const NO_DATA_CLASS: &str = "02";
const SQLCODE_NO_DATA: i32 = 100;
/// \[DESIGN\] The SQLSTATE of a notice that a Reset or a ChangeUser rolled
/// back the transaction it found active: a subclass of warning this
/// implementation defines.
pub(super) const ROLLED_BACK: &str = "01T01";

/// [ISO/IEC 9075:1992 §22.1 Table 23] Each subclass of warning, and its
/// subcondition's name.
const SUBCONDITIONS: [(&str, &str); 11] = [
    ("01000", "warning"),
    ("01001", "cursor operation conflict"),
    ("01002", "disconnect error"),
    ("01003", "null value eliminated in set function"),
    ("01004", "string data, right truncation"),
    ("01005", "insufficient item descriptor areas"),
    ("01006", "privilege not revoked"),
    ("01007", "privilege not granted"),
    ("01008", "implicit zero-bit padding"),
    ("01009", "search condition too long for information schema"),
    ("0100A", "query expression too long for information schema"),
];

/// The words a notice of the warning `sqlstate` says, of a statement that
/// completed with it.
fn named(sqlstate: &str) -> String {
    SUBCONDITIONS.iter().find(|(state, _)| *state == sqlstate).map_or_else(
        || format!("the statement completed with warning {sqlstate}"),
        |(_, name)| format!("the statement completed with warning: {name}"),
    )
}

/// A notice of `sqlstate` saying `message`, of `severity`.
fn notice(sqlstate: &str, message: String, severity: &str) -> Notice {
    let diagnostics = Diagnostics { severity: Some(severity.to_owned()), ..Diagnostics::default() };
    Notice { sqlstate: sqlstate.to_owned(), message, diagnostics }
}

/// [ISO/IEC 9075:1992 §22.1, Table 24] `notice`, a condition a statement
/// completed with, as a Complete carries it: SQLCODE 100 of class 02, 0
/// of a warning; of a transaction `active` or not.
pub(super) fn condition(notice: Notice, active: bool) -> Failure {
    let sqlcode = if notice.sqlstate.starts_with(NO_DATA_CLASS) { SQLCODE_NO_DATA } else { 0_i32 };
    let Notice { sqlstate, message, diagnostics } = notice;
    Failure { sqlcode, sqlstate, message, rule: None, position: None, active, diagnostics: Some(Box::new(diagnostics)) }
}

/// The notice of the server's own that the transaction it found active
/// was rolled back by `what`, a Reset or a ChangeUser.
pub(super) fn rolled_back(what: &str) -> Reply {
    Reply::Notice(notice(ROLLED_BACK, format!("[DESIGN] {what} rolled back the transaction that was active"), NOTICE))
}

/// The notices of the statement `session` ran, whose last reply is
/// `last`: each SIGNAL's, taken, then its completion's warning, unless a
/// SIGNAL's is that one.
pub(super) fn raised(session: &Session, last: &Reply) -> Vec<Notice> {
    let notes = session.canceller().served().take_notes();
    let completed = match last {
        Reply::Complete(c) => Some(c.sqlstate.as_str()).filter(|s| s.starts_with(WARNING_CLASS)),
        Reply::Error(_)
        | Reply::Challenge(_)
        | Reply::Ready { .. }
        | Reply::Prepared { .. }
        | Reply::Description(_)
        | Reply::Rows(_)
        | Reply::Suspended
        | Reply::Lob(_)
        | Reply::Notice(_)
        | Reply::Status { .. }
        | Reply::Pong
        | Reply::Debug(_)
        | Reply::Event(_) => None,
    };
    let signalled = |state: &str| notes.iter().any(|(s, _): &(SqlState, String)| s.as_str() == state);
    let own = completed.filter(|state| !signalled(state)).map(|state| notice(state, named(state), WARNING));
    let mut all: Vec<Notice> = notes.iter().map(|(state, message)| notice(state.as_str(), message.clone(), WARNING)).collect();
    all.extend(own);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] [ISO/IEC 9075:1992 §22.1 Table 23] RULES.md D187:
    /// a warning's notice names its subclass as the standard does; one of
    /// a subclass it does not name says its SQLSTATE.
    #[test]
    fn a_warning_is_named_by_its_subclass() {
        assert_eq!(named("01004"), "the statement completed with warning: string data, right truncation");
        assert_eq!(named("0100F"), "the statement completed with warning 0100F");
    }
}
