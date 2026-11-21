//! \[EXTENSION\] The conditions a plug-in's call fails with (RULES.md
//! D172): of class 38, external routine exception [ISO/IEC 9075-2:2023
//! §24.1, Table 38], its subclasses beginning with T this
//! implementation's, as a subclass beginning with a letter from I to Z is
//! implementation-defined [§24.1]. A program's own error keeps its
//! SQLSTATE if it is of a class a routine may raise, and is 38000
//! otherwise.

use crate::error::{Code, SqlError, SqlState};

/// [ISO/IEC 9075-2:2023 §24.1, Table 38] External routine exception, no
/// subclass: a program's error of a state it may not raise.
pub const EXTERNAL_ROUTINE_EXCEPTION: SqlState = SqlState::of_bytes(*b"38000");
/// \[DESIGN\] The program ended, crashed or closed a pipe.
pub const ENDED: SqlState = SqlState::of_bytes(*b"38T01");
/// \[DESIGN\] The call ran past PLUGINS.CALL_TIMEOUT_MS.
pub const TIMED_OUT: SqlState = SqlState::of_bytes(*b"38T02");
/// \[DESIGN\] The program broke protocol 1.
pub const VIOLATION: SqlState = SqlState::of_bytes(*b"38T03");
/// \[DESIGN\] The program sent more than PLUGINS.MAX_MESSAGE_BYTES in a
/// frame or PLUGINS.MAX_CALL_BYTES in a call.
pub const TOO_LONG: SqlState = SqlState::of_bytes(*b"38T04");
/// \[DESIGN\] The program cannot be started: its directory is not allowed,
/// its executable is missing, or it fails to start.
pub const UNAVAILABLE: SqlState = SqlState::of_bytes(*b"38T05");
/// \[DESIGN\] The manifest breaks a rule of its format.
pub const INVALID_MANIFEST: SqlState = SqlState::of_bytes(*b"38T06");
/// \[DESIGN\] A change a plug-in's table or index was given could not be
/// told made or undone: the object is damaged until the database is
/// opened again and recovers it (RULES.md D225, D227).
pub const BROKEN: SqlState = SqlState::of_bytes(*b"38T10");
/// A host plug-in failed, and its node stopped until it starts again
/// (RULES.md D229).
pub const HOST_FAILED: SqlState = SqlState::of_bytes(*b"38T11");

/// The classes a program's error keeps: data exception, integrity
/// constraint violation, external routine exception, external routine
/// invocation exception, syntax error or access rule violation, and SQL
/// routine exception [ISO/IEC 9075-2:2023 §24.1, Table 38].
const KEPT_CLASSES: [&str; 6] = ["22", "23", "38", "39", "42", "2F"];

/// A call's failure of state `state`, why it failed `why`.
fn failed(state: SqlState, why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] {why}")).with_state(state)
}

/// 38T01: the program ended, `why`.
pub fn ended(why: &str) -> SqlError {
    failed(ENDED, &format!("the plug-in process ended: {why}"))
}

/// 38T02: the call ran past `ms` milliseconds.
pub fn timed_out(ms: u64) -> SqlError {
    failed(TIMED_OUT, &format!("the plug-in call ran past PLUGINS.CALL_TIMEOUT_MS, {ms} ms, and its process was stopped"))
}

/// 38T02: the program started gave no WELCOME within `ms` milliseconds.
pub fn unwelcomed(ms: u64) -> SqlError {
    failed(
        TIMED_OUT,
        &format!(
            "the plug-in gave no WELCOME within {ms} ms of its start, PLUGINS.CALL_TIMEOUT_MS or the least a start is given if more, and its process was stopped"
        ),
    )
}

/// 38T03: the program broke the protocol, `why`.
pub fn violation(why: &str) -> SqlError {
    failed(VIOLATION, &format!("the plug-in broke protocol 1: {why}"))
}

/// 38T04: a frame of `length` bytes, past `max`.
pub fn too_long(length: u64, max: u64) -> SqlError {
    failed(
        TOO_LONG,
        &format!("the plug-in sent {length} bytes, past the limit of {max} (PLUGINS.MAX_MESSAGE_BYTES, PLUGINS.MAX_CALL_BYTES)"),
    )
}

/// 38T05: the program cannot be started, `why`.
pub fn unavailable(why: &str) -> SqlError {
    failed(UNAVAILABLE, &format!("the plug-in cannot be started: {why}"))
}

/// 38T07: a process of `extension` stopped while the transaction wrote
/// its object `object`.
pub fn restarted(extension: &str, object: &str) -> SqlError {
    let why = format!("[EXTENSION] a process of extension {extension} stopped while the transaction wrote {object}, so it fails");
    SqlError::new(Code::Conflict, why)
}

/// 38T10: the object `object` could not be told whole, `why`.
pub fn broken(object: &str, why: &str) -> SqlError {
    let why = format!("[EXTENSION] the plug-in's {object} is damaged until the database is opened again: {why}");
    SqlError::new(Code::Damaged, why).with_state(BROKEN)
}

/// 38T06: the manifest breaks a rule, `why`.
pub fn invalid_manifest(why: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] the plug-in manifest is invalid: {why}")).with_state(INVALID_MANIFEST)
}

/// The error a program's ERROR of `state` and `message` raises, of the
/// object `object`: its state kept if of a class a routine may raise,
/// else 38000; its code the class's.
pub fn raised(object: &str, state: &str, message: &str) -> SqlError {
    let kept = SqlState::parse(state).filter(|s| KEPT_CLASSES.contains(&s.class()));
    let chosen = kept.unwrap_or(EXTERNAL_ROUTINE_EXCEPTION);
    let code = match chosen.class() {
        "23" => Code::Constraint,
        "42" => Code::SyntaxRule,
        _ => Code::Data,
    };
    SqlError::new(code, format!("[EXTENSION] {object} failed: {message}")).with_state(chosen)
}

/// 38T11: the host plug-in `plugin` failed, `why`, and the node stopped.
pub fn host_failed(plugin: &str, why: &str) -> SqlError {
    failed(HOST_FAILED, &format!("the host plug-in {plugin} failed, so the node stopped until it starts again: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] RULES.md D172: a program's error keeps its
    /// SQLSTATE of class 22, 23, 38, 39, 42 or 2F, with the SQLCODE of
    /// the class; any other, or one no SQLSTATE, is 38000, -301.
    #[test]
    fn a_programs_error_keeps_its_state_only_of_the_classes_a_routine_raises() {
        let e = raised("TWICE", "22012", "division by zero");
        assert_eq!((e.sqlstate().as_str(), e.sqlcode()), ("22012", -301_i32));
        assert_eq!(raised("F", "23505", "dup").sqlcode(), -201_i32);
        assert_eq!(raised("F", "42000", "bad").sqlcode(), -103_i32);
        assert_eq!(raised("F", "2F000", "r").sqlstate().as_str(), "2F000");
        for other in ["08006", "40001", "0000", "zz", "00000"] {
            let mapped = raised("F", other, "x");
            assert_eq!((mapped.sqlstate(), mapped.sqlcode()), (EXTERNAL_ROUTINE_EXCEPTION, -301_i32), "{other}");
        }
        assert!(raised("F", "38001", "m").msg.contains("F failed: m"));
    }

    /// Verifies: [EXTENSION] RULES.md D172: each condition of the runtime
    /// has its subclass of class 38, the manifest's SQLCODE -103 and the
    /// others -301.
    #[test]
    fn the_runtimes_conditions_are_of_class_38() {
        let all = [ended("x"), timed_out(5), violation("x"), too_long(9, 8), unavailable("x")];
        let states: Vec<SqlState> = all.iter().map(SqlError::sqlstate).collect();
        assert_eq!(states, [ENDED, TIMED_OUT, VIOLATION, TOO_LONG, UNAVAILABLE]);
        assert!(all.iter().all(|e| e.sqlcode() == -301_i32));
        let m = invalid_manifest("no NAME");
        assert_eq!((m.sqlstate().as_str(), m.sqlcode()), ("38T06", -103_i32));
    }
}
