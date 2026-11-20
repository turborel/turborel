//! VECTOR (RULES.md D93) through SQL: its type and text forms, the
//! refusals of order and equality, its distances and operators, its
//! functions and aggregates, distance functions of SQL and metrics
//! registered, and its values kept by recovery. It is built in, so no
//! test registers it.

use turborel::db::{Database, Session};

use crate::common::{refused, rows};

mod distance_functions;
mod distances;
mod functions;
mod recovery;
mod refusals;
mod text_lobs;
mod types;

/// A database whose schema A has table `table` (`columns`), and a
/// session of A.
pub fn table(columns: &str) -> (Database, Session) {
    let db = Database::new();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T ({columns})")).unwrap();
    let a = db.session("A");
    (db, a)
}

/// The rows of `sql`, which must succeed.
pub fn got(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|code| panic!("{sql}: {code:?}"))
}

/// The one value `sql` gives, a number.
pub fn number(s: &Session, sql: &str) -> f64 {
    let found = got(s, sql);
    let [one] = found.as_slice() else { panic!("{sql}: {found:?}") };
    one.parse().unwrap_or_else(|_| panic!("{sql}: {one}"))
}

/// Whether `x` is `want`, to rounding.
pub fn near(x: f64, want: f64) -> bool {
    (x - want).abs() <= 1e-6 * want.abs().max(1.0)
}

/// The SQLCODE and SQLSTATE of the error `sql` raises.
pub fn error(s: &Session, sql: &str) -> (i32, String) {
    let (code, state, _) = refused(s, sql);
    (code, state)
}
