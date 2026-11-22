//! [ISO/IEC 9075-2:2023 §7.6] Lateral derived tables (RULES.md D778,
//! D779): LATERAL ( `<query expression>` ), its query reading the table
//! references before it in its FROM clause, computed again for each of
//! their rows; one that reads none is a derived table; and a key compared
//! with the lateral's columns probes the index once for each of the rows.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;

use crate::common::{rows, sorted};

/// Schema P: N, keyed (W, D, O), of orders 1 to 1000 of districts 1 to 3 of
/// warehouse 1, V the order mod 4; and K, the districts 1 to 4, the last
/// of no orders.
fn orders() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION P
           CREATE TABLE N (W INTEGER NOT NULL, D INTEGER NOT NULL, O INTEGER NOT NULL, V INTEGER, PRIMARY KEY (W, D, O))
           CREATE TABLE K (D INTEGER NOT NULL PRIMARY KEY)",
    )
    .unwrap();
    let s = db.session("P");
    let values: Vec<String> = (1..=3).flat_map(|d| (1..=1000).map(move |o| format!("(1, {d}, {o}, {})", o % 4))).collect();
    for chunk in values.chunks(500) {
        s.run(&format!("INSERT INTO N VALUES {}", chunk.join(", "))).unwrap();
    }
    s.run("INSERT INTO K VALUES (1), (2), (3), (4)").unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// The physical plan of `query`.
fn physical(s: &Session, query: &str) -> String {
    s.explain(query, &Params::new()).unwrap().split("-- physical:").nth(1).unwrap_or_default().to_string()
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 SR5, SR10, SR20a]; [DESIGN] RULES.md
/// D778: a lateral derived table after a comma or CROSS JOIN gives its
/// query's rows for each row before it, none of a row its query has no
/// rows of; its columns are named as its query names them or by its
/// derived column list; an ORDER BY of its query says which rows FETCH
/// keeps.
#[test]
fn a_lateral_query_is_computed_for_each_row_before_it() {
    let db = orders();
    let s = db.session("P");
    let least = "SELECT K.D, M.O FROM K, LATERAL (SELECT MIN(O) FROM N WHERE W = 1 AND N.D = K.D) AS M (O)";
    assert_eq!(sorted(&s, least), ["1,1", "2,1", "3,1", "4,NULL"]);
    let named = "SELECT K.D, M.HI FROM K CROSS JOIN LATERAL (SELECT MAX(O) AS HI FROM N WHERE N.D = K.D) AS M";
    assert_eq!(sorted(&s, named), ["1,1000", "2,1000", "3,1000", "4,NULL"]);
    let top = "SELECT K.D, T.O FROM K, LATERAL (SELECT O FROM N WHERE N.D = K.D AND V = 0 ORDER BY O DESC FETCH FIRST 2 ROWS ONLY) AS T";
    assert_eq!(sorted(&s, top), ["1,1000", "1,996", "2,1000", "2,996", "3,1000", "3,996"]);
    let joined = "SELECT X.D, T.C FROM UNNEST(ARRAY[2, 3]) AS X (D), LATERAL (SELECT COUNT(*) FROM N WHERE N.D = X.D AND O > 995) AS T (C)";
    assert_eq!(sorted(&s, joined), ["2,5", "3,5"]);
    // Of an earlier lateral's columns too.
    let chained = "SELECT K.D, A.O, B.O FROM K, LATERAL (SELECT MIN(O) FROM N WHERE N.D = K.D) AS A (O), \
                   LATERAL (SELECT O FROM N WHERE N.D = K.D AND O = A.O + 1) AS B (O)";
    assert_eq!(sorted(&s, chained), ["1,1,2", "2,1,2", "3,1,2"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 SR20a, §7.10 GR1d]; [DESIGN]
/// RULES.md D778: of a LEFT JOIN, a row before it its query has no rows of
/// is kept, the lateral's columns null; a lateral that reads nothing
/// before it gives the rows a derived table of its query gives.
#[test]
fn a_lateral_of_a_left_join_keeps_its_rows_and_one_of_nothing_before_it_is_a_derived_table() {
    let db = orders();
    let s = db.session("P");
    let left = "SELECT K.D, T.O FROM K LEFT JOIN LATERAL (SELECT O FROM N WHERE N.D = K.D AND O > 999) AS T ON TRUE";
    assert_eq!(sorted(&s, left), ["1,1000", "2,1000", "3,1000", "4,NULL"]);
    let on = "SELECT K.D, T.O FROM K LEFT JOIN LATERAL (SELECT O FROM N WHERE N.D = K.D AND O > 998) AS T ON T.O = 999 + K.D - 1";
    assert_eq!(sorted(&s, on), ["1,999", "2,1000", "3,NULL", "4,NULL"]);
    let plain = "SELECT K.D, T.C FROM K, LATERAL (SELECT COUNT(*) FROM N WHERE V = 1) AS T (C)";
    let derived = "SELECT K.D, T.C FROM K, (SELECT COUNT(*) FROM N WHERE V = 1) AS T (C)";
    assert_eq!(sorted(&s, plain), sorted(&s, derived));
    assert_eq!(physical(&s, plain), physical(&s, derived));
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 F, SR20a, §7.10 SR4, §6.9 SR7]: a
/// lateral derived table has a correlation name; it reads no table
/// reference after it, nor its own first operand of a RIGHT or FULL join;
/// a set function of its query is over no outer reference to the FROM row;
/// LATERAL is followed by a subquery.
#[test]
fn a_lateral_derived_table_is_refused_where_the_standard_refuses_it() {
    let db = orders();
    let s = db.session("P");
    assert_eq!(rows(&s, "SELECT * FROM K, LATERAL (SELECT O FROM N WHERE N.D = K.D)"), Err(Code::Syntax));
    assert_eq!(rows(&s, "SELECT * FROM K, LATERAL (N) AS T"), Err(Code::Syntax));
    let after = "SELECT * FROM LATERAL (SELECT O FROM N WHERE N.D = K.D) AS T, K";
    assert_eq!(rows(&s, after), Err(Code::SyntaxRule));
    let right = "SELECT * FROM K RIGHT JOIN LATERAL (SELECT O FROM N WHERE N.D = K.D) AS T ON TRUE";
    assert_eq!(rows(&s, right), Err(Code::SyntaxRule));
    let aggregated = "SELECT * FROM K, LATERAL (SELECT SUM(K.D) FROM N) AS T (S)";
    assert_eq!(rows(&s, aggregated), Err(Code::SyntaxRule));
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 SR20a, §8.4 GR3]; [DESIGN] RULES.md
/// D779: the least key of each district, a lateral's set function of the
/// rows before it, reads the index once for each, not every row of the
/// districts; a DELETE of the rows of those keys deletes one of each and
/// probes them, its IN a semi join; an IN of a lateral of many rows, whose
/// multiset may outgrow its bound, is no semi join, nor one of a lateral
/// whose query can raise an error. MC/DC of an UNNEST's subquery raising
/// is shown by the pairs of one row at most against many, and of a plan
/// raising nothing against one that can.
#[test]
fn a_lateral_set_function_of_a_key_reads_one_entry_for_each_row_before_it() {
    let db = orders();
    let s = db.session("P");
    let least = "SELECT X.D, M.O FROM UNNEST(ARRAY[1, 2, 3]) AS X (D), \
                 LATERAL (SELECT MIN(O) FROM N WHERE W = 1 AND N.D = X.D) AS M (O)";
    let plan = s.explain_analyze(least, &Params::new()).unwrap();
    assert!(plan.contains("entries=3,") && !plan.contains("SeqScan"), "{plan}");
    let delete = format!("DELETE FROM N WHERE W = 1 AND (D, O) IN ({least})");
    let plan = s.explain_analyze(&delete, &Params::new()).unwrap();
    assert!(plan.contains("IndexNestedLoop P.N") && !plan.contains("SeqScan"), "{plan}");
    assert_eq!(sorted(&s, "SELECT D, MIN(O) FROM N GROUP BY D"), ["1,2", "2,2", "3,2"]);
    s.rollback().unwrap();
    let probed = format!("SELECT D, O FROM N WHERE W = 1 AND (D, O) IN ({least})");
    assert!(physical(&s, &probed).contains("IndexNestedLoop P.N"));
    assert!(s.explain(&probed, &Params::new()).unwrap().contains("-- rules: semi_joins"));
    assert_eq!(sorted(&s, &probed), ["1,1", "2,1", "3,1"]);
    let many = "SELECT D, O FROM N WHERE W = 1 AND (D, O) IN (SELECT X.D, M.O FROM UNNEST(ARRAY[1, 2]) AS X (D), \
                LATERAL (SELECT O FROM N WHERE N.D = X.D AND O < 3) AS M (O))";
    assert!(!s.explain(many, &Params::new()).unwrap().contains("-- rules: semi_joins"));
    assert_eq!(sorted(&s, many), ["1,1", "1,2", "2,1", "2,2"]);
    let raising = "SELECT D, O FROM N WHERE W = 1 AND (D, O) IN (SELECT X.D, M.O FROM UNNEST(ARRAY[1, 2]) AS X (D), \
                   LATERAL (SELECT MIN(O / (V + 1)) FROM N WHERE N.D = X.D) AS M (O))";
    assert!(!s.explain(raising, &Params::new()).unwrap().contains("-- rules: semi_joins"));
}
