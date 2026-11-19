//! \[EXTENSION\] Where the binder meets the graph pattern matching
//! language: a graph predicate or function outside a GRAPH_TABLE, which
//! only a GRAPH_TABLE's conditions and columns may have
//! (`binder/graph.rs`).

use crate::error::{Code, SqlError};

/// The error of a graph predicate or graph function written outside a
/// GRAPH_TABLE: its variables are a graph pattern's, which no other
/// query declares.
pub(super) fn outside_graph_table() -> SqlError {
    SqlError::new(
        Code::SyntaxRule,
        "[EXTENSION] a graph predicate or graph function is written only in a GRAPH_TABLE, of its pattern's variables",
    )
}

#[cfg(test)]
mod tests {
    use crate::db::Database;

    /// The SQLCODE and message `sql` fails with in a session of a new
    /// database.
    fn failed(sql: &str) -> (i32, String) {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION G CREATE TABLE T (X INTEGER)").unwrap();
        let s = db.session("G");
        let e = s.open_cursor(sql).map(|_| panic!("{sql}: accepted")).unwrap_err();
        (e.sqlcode(), e.msg)
    }

    /// Verifies: [EXTENSION] [P1 §4.7] a graph predicate written outside
    /// a GRAPH_TABLE is refused with SQLCODE -103, and a routine named as
    /// a graph function is still a routine there.
    #[test]
    fn graph_predicates_are_refused_outside_graph_table_4_7() {
        for sql in [
            "SELECT X FROM T WHERE X IS SOURCE OF Y",
            "SELECT X FROM T WHERE X IS LABELED A",
            "SELECT X FROM T WHERE X IS NOT DIRECTED",
            "SELECT X FROM T WHERE X:A|B",
        ] {
            let (code, msg) = failed(sql);
            assert_eq!(code, -103_i32, "{sql}: {msg}");
            assert!(msg.contains("only in a GRAPH_TABLE"), "{sql}: {msg}");
        }
        let (_, msg) = failed("SELECT SAME(X, X) FROM T");
        assert!(!msg.contains("GRAPH_TABLE"), "SAME is a routine's name outside a GRAPH_TABLE: {msg}");
    }
}
