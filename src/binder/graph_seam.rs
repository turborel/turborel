//! \[EXTENSION\] Where the binder meets the graph pattern matching
//! language: a graph predicate or function outside a GRAPH_TABLE, which
//! only a GRAPH_TABLE's conditions and columns may have, and GRAPH_TABLE
//! itself, which the parser reads and checks and this binder does not
//! yet bind.

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

/// The error of a GRAPH_TABLE, read and checked but not bound. This seam
/// is temporary, until the binder binds a GRAPH_TABLE.
pub(super) fn not_yet_bound() -> SqlError {
    SqlError::new(Code::SyntaxRule, "[EXTENSION] GRAPH_TABLE is not yet bound")
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

    /// Verifies: [EXTENSION] [P1 §3] a GRAPH_TABLE the parser reads and
    /// checks reaches the binder, which refuses it with SQLCODE -103, as
    /// it binds none yet.
    #[test]
    fn graph_table_reaches_the_binder_3() {
        let (code, msg) = failed("SELECT * FROM GRAPH_TABLE (G MATCH (a)-[e]->(b) COLUMNS (a.x AS X)) AS GT");
        assert_eq!(code, -103_i32);
        assert!(msg.contains("GRAPH_TABLE is not yet bound"), "{msg}");
        let (checked, why) = failed("SELECT * FROM GRAPH_TABLE (G MATCH (a)->*(b) COLUMNS (a.x AS X))");
        assert_eq!(checked, -103_i32);
        assert!(why.contains("GRF-0031"), "the parser checks first: {why}");
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
