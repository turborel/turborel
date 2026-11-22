//! Unit tests: which statements of a program kept keep their plans
//! (RULES.md D406, D792, R-TEST-8).

use std::collections::HashSet;

use super::*;
use crate::parser::Parser;

/// `text`, parsed as a statement of a procedure declaring N and X.
fn parsed(text: &str) -> SqlStatement {
    let declared: HashSet<String> = ["N", "X"].map(str::to_owned).into_iter().collect();
    Parser::for_procedure(text, false, declared).unwrap().sql_statement().unwrap()
}

/// Verifies: [DESIGN] RULES.md D792: a SELECT ... INTO whose one table
/// reference is the delta table of a searched UPDATE, a searched DELETE or
/// an INSERT keeps that statement's plan, as the statement alone does.
/// MC/DC pairs of keeping: the delta table of a searched UPDATE, kept; of
/// a MERGE, not; a SELECT ... INTO of a base table, not; two table
/// references, one a delta table, not.
#[test]
fn a_select_into_keeps_the_plan_of_its_delta_tables_change() {
    let update = "SELECT Q INTO X FROM FINAL TABLE (UPDATE S SET Q = Q - N WHERE W = N)";
    let delete = "SELECT Q INTO X FROM OLD TABLE (DELETE FROM S WHERE W = N)";
    let insert = "SELECT Q INTO X FROM FINAL TABLE (INSERT INTO S VALUES (N, N, N))";
    for kept in [update, delete, insert, "UPDATE S SET Q = N", "DELETE FROM S WHERE W = N"] {
        assert!(keeps_plan(&parsed(kept)), "{kept}");
    }
    let merge = "SELECT Q INTO X FROM FINAL TABLE (MERGE INTO S USING T ON S.W = T.W WHEN MATCHED THEN UPDATE SET Q = N)";
    let base = "SELECT Q INTO X FROM S WHERE W = N";
    let two = "SELECT Q INTO X FROM FINAL TABLE (UPDATE S SET Q = N), T";
    for planned in [merge, base, two] {
        assert!(!keeps_plan(&parsed(planned)), "{planned}");
    }
}
