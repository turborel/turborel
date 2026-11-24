//! \[DESIGN\] An outer join made the join it is by a WHERE conjunct over
//! both operands' columns that is never true of the rows it gives nulls
//! (RULES.md D1061) [ISO/IEC 9075:1992 §7.5 GR5].

use turborel::db::Database;
use turborel::optimizer::PUSH_FILTER_INTO_OUTER_JOIN;
use turborel::params::Params;

use crate::common::{set_all_rules, sorted};

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.5 GR5; X3.135-1986 §5.11
/// GR2] [DESIGN] RULES.md D1061: a WHERE comparison of a column of each
/// operand of a LEFT, RIGHT or FULL join, plain, of arithmetic, negated,
/// or of a CASE that is null of the nulls, makes the join inner, as it is
/// unknown of every row the join gives nulls, and so where the join is an
/// operand of the product the WHERE filters; one a disjunct, a COALESCE
/// or a CASE may make true of them, or one of the preserved operand's
/// columns, leaves the join. Each query gives the rows it gives with no
/// rule.
#[test]
fn a_comparison_of_both_operands_makes_an_outer_join_inner_7_5() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION O CREATE TABLE T (ID INTEGER, A INTEGER) CREATE TABLE U (ID INTEGER, B INTEGER)
         CREATE TABLE V (X INTEGER)",
    )
    .unwrap();
    let s = db.session("O");
    s.run("INSERT INTO V VALUES (3), (6)").unwrap();
    s.run("INSERT INTO T VALUES (1, 5), (2, 1), (3, 9), (NULL, 4)").unwrap();
    s.run("INSERT INTO U VALUES (1, 7), (1, 2), (2, NULL), (4, 8)").unwrap();
    s.commit().unwrap();
    let lj = "SELECT T.ID, A, B FROM T LEFT JOIN U ON T.ID = U.ID";
    let cases: [(String, &[&str], bool); 11] = [
        ("SELECT T.ID, A, B FROM V, T LEFT JOIN U ON T.ID = U.ID WHERE X < B".into(), &["1,5,7", "1,5,7"], true),
        (
            "SELECT T.ID, A, B FROM V, T LEFT JOIN U ON T.ID = U.ID WHERE X < A".into(),
            &["1,5,2", "1,5,7", "3,9,NULL", "3,9,NULL", "NULL,4,NULL"],
            false,
        ),
        (format!("{lj} WHERE A < B"), &["1,5,7"], true),
        (format!("{lj} WHERE A < B + 1"), &["1,5,7"], true),
        (format!("{lj} WHERE NOT (A < B)"), &["1,5,2"], true),
        (format!("{lj} WHERE A < CASE WHEN U.ID IS NULL THEN NULL ELSE B END"), &["1,5,7"], true),
        ("SELECT T.ID, A, B FROM U RIGHT JOIN T ON T.ID = U.ID WHERE A < B".into(), &["1,5,7"], true),
        ("SELECT T.ID, A, B FROM T FULL JOIN U ON T.ID = U.ID WHERE A < B".into(), &["1,5,7"], true),
        (format!("{lj} WHERE A < B OR A = 9"), &["1,5,7", "3,9,NULL"], false),
        (format!("{lj} WHERE A < COALESCE(B, 6)"), &["1,5,7", "2,1,NULL", "NULL,4,NULL"], false),
        (format!("{lj} WHERE A < CASE WHEN U.ID IS NULL THEN 6 ELSE B END"), &["1,5,7", "NULL,4,NULL"], false),
    ];
    for (sql, want, inner) in &cases {
        let plan = s.explain(sql, &Params::new()).unwrap();
        let physical = plan.split("-- physical:").nth(1).unwrap();
        assert_eq!(!physical.contains("OuterJoin"), *inner, "{sql}: {plan}");
        assert_eq!(plan.contains(PUSH_FILTER_INTO_OUTER_JOIN), *inner, "{sql}: {plan}");
        assert_eq!(sorted(&s, sql), want.to_vec(), "{sql}");
        set_all_rules(&db, false);
        assert_eq!(sorted(&s, sql), want.to_vec(), "{sql}: with no rule");
        set_all_rules(&db, true);
    }
}
