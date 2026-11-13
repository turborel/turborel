//! Partitioned join tables [ISO/IEC 9075-2:2023 §7.10, Feature F403]: an
//! operand of an outer join PARTITION BY some of its columns, the other
//! operand joined with it partition by partition, a row unmatched in a
//! partition given its values; LEFT, RIGHT and FULL, either operand or
//! both; nulls, collations, arrays, empty operands; each Syntax Rule; and
//! the join by batches, row by row and past the work area.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{Counted, both_ways, exec, fetched, rows, sorted};

/// Schema A: T, of dates 1 to 3; S, of sales of products 'a', 'b' and a
/// null one on some of them; U, a value of each product; E, empty.
fn sales() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (D INTEGER)
           CREATE TABLE S (P VARCHAR(5), D INTEGER, AMOUNT INTEGER)
           CREATE TABLE U (P VARCHAR(5), Q INTEGER)
           CREATE TABLE E (P VARCHAR(5), D INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1), (2), (3)").unwrap();
    a.run("INSERT INTO S VALUES ('a', 1, 10), ('a', 3, 30), ('b', 2, 20), (NULL, 1, 5)").unwrap();
    a.run("INSERT INTO U VALUES ('a', 7), ('b', 8)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The rows of `sql`, sorted.
fn of(s: &Session, sql: &str) -> Vec<String> {
    sorted(s, sql)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.10 GR4, GR5b, GR5c] a
/// LEFT join whose second operand is partitioned gives each row of the
/// first once for each partition: with the rows of that partition it
/// matches, else with nulls but for the partitioning columns, the
/// partition's [GR4h]. The null product is a partition of its own, its
/// rows' values not distinct [GR4c i 2)]. A RIGHT join whose first operand
/// is partitioned is its mirror. A NATURAL join's common columns are
/// coalesced over it as over any.
#[test]
fn gaps_are_filled_by_partition_7_10() {
    let (_db, a) = sales();
    let filled = ["NULL,1,5", "NULL,2,NULL", "NULL,3,NULL", "a,1,10", "a,2,NULL", "a,3,30", "b,1,NULL", "b,2,20", "b,3,NULL"];
    assert_eq!(of(&a, "SELECT S.P, T.D, S.AMOUNT FROM T LEFT JOIN S PARTITION BY (P) ON T.D = S.D"), filled);
    assert_eq!(of(&a, "SELECT S.P, T.D, S.AMOUNT FROM S PARTITION BY (S.P) RIGHT OUTER JOIN T ON T.D = S.D"), filled);
    let natural = ["1,NULL,5", "1,a,10", "1,b,NULL", "2,NULL,NULL", "2,a,NULL", "2,b,20", "3,NULL,NULL", "3,a,30", "3,b,NULL"];
    assert_eq!(of(&a, "SELECT * FROM T NATURAL LEFT JOIN S PARTITION BY (P)"), natural);
    let chained = "SELECT S.P, T.D, U.Q FROM T LEFT JOIN S PARTITION BY (P) ON T.D = S.D LEFT JOIN U ON S.P = U.P";
    assert_eq!(of(&a, chained).len(), 9, "a partitioned join joined again");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.10 GR4, GR5d] a FULL join
/// whose second operand is partitioned gives the matched rows, each row of
/// the first unmatched in a partition with that partition's values
/// [GR4h], and each row of the second that no row matches with nulls
/// [GR4i]; one whose operands are both partitioned, each operand's rows
/// unmatched in each of the other's partitions with that partition's
/// values.
#[test]
fn full_joins_fill_both_ways_7_10() {
    let (_db, a) = sales();
    let one = "SELECT * FROM T FULL JOIN S PARTITION BY (P) ON T.D = S.D AND S.AMOUNT > 10";
    assert_eq!(
        of(&a, one),
        [
            "1,NULL,NULL,NULL",
            "1,a,NULL,NULL",
            "1,b,NULL,NULL",
            "2,NULL,NULL,NULL",
            "2,a,NULL,NULL",
            "2,b,2,20",
            "3,NULL,NULL,NULL",
            "3,a,3,30",
            "3,b,NULL,NULL",
            "NULL,NULL,1,5",
            "NULL,a,1,10",
        ]
    );
    let both = "SELECT * FROM S PARTITION BY (P) FULL JOIN S AS S2 PARTITION BY (S2.P) ON S.D = S2.D";
    assert_eq!(
        of(&a, both),
        [
            "NULL,1,5,NULL,1,5",
            "NULL,1,5,a,1,10",
            "NULL,1,5,b,NULL,NULL",
            "NULL,NULL,NULL,a,3,30",
            "NULL,NULL,NULL,b,2,20",
            "a,1,10,NULL,1,5",
            "a,1,10,a,1,10",
            "a,1,10,b,NULL,NULL",
            "a,3,30,NULL,NULL,NULL",
            "a,3,30,a,3,30",
            "a,3,30,b,NULL,NULL",
            "a,NULL,NULL,b,2,20",
            "b,2,20,NULL,NULL,NULL",
            "b,2,20,a,NULL,NULL",
            "b,2,20,b,2,20",
            "b,NULL,NULL,NULL,1,5",
            "b,NULL,NULL,a,1,10",
            "b,NULL,NULL,a,3,30",
        ]
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.10 GR4b i 1), GR4c i 1),
/// NOTE 239, NOTE 240] an empty partitioned operand has no partitions, so
/// no row of the other operand is extended with one: a LEFT or FULL join
/// of an empty second operand partitioned, and a FULL join of an empty
/// first, are empty. A FULL join of an empty first operand not
/// partitioned gives each row of the second.
#[test]
fn empty_partitioned_operands_give_nothing_7_10() {
    let (_db, a) = sales();
    for empty in [
        "SELECT * FROM T LEFT JOIN E PARTITION BY (P) ON T.D = E.D",
        "SELECT * FROM T FULL JOIN E PARTITION BY (P) ON T.D = E.D",
        "SELECT * FROM E PARTITION BY (P) FULL JOIN T ON T.D = E.D",
    ] {
        assert_eq!(of(&a, empty), Vec::<String>::new(), "{empty}");
    }
    assert_eq!(of(&a, "SELECT * FROM E FULL JOIN S PARTITION BY (P) ON E.D = S.D").len(), 4);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.10 GR4c i 2), §8.15,
/// §9.11] partitions are of values not distinct: under CASE_INSENSITIVE,
/// 'a' and 'A' are one partition, and under pad space 'x' and 'x '; a
/// matched row keeps its own value, and an unmatched row is given one of
/// its partition's. An array column partitions too, as arrays compare for
/// equality; so do two columns.
#[test]
fn partitions_are_of_values_not_distinct_7_10() {
    let (_db, a) = sales();
    a.run("CREATE TABLE C (P VARCHAR(5) COLLATE CASE_INSENSITIVE, D INTEGER)").unwrap();
    a.run("INSERT INTO C VALUES ('a', 1), ('A', 2), ('b', 1)").unwrap();
    let collated = "SELECT T.D, UPPER(C.P), C.D FROM T LEFT JOIN C PARTITION BY (P) ON T.D = C.D";
    assert_eq!(of(&a, collated), ["1,A,1", "1,B,1", "2,A,2", "2,B,NULL", "3,A,NULL", "3,B,NULL"]);
    assert_eq!(of(&a, "SELECT C.P FROM T LEFT JOIN C PARTITION BY (P) ON T.D = C.D WHERE C.D IS NOT NULL"), ["A", "a", "b"]);
    a.run("CREATE TABLE W (P VARCHAR(5), D INTEGER)").unwrap();
    a.run("INSERT INTO W VALUES ('x', 1), ('x ', 2)").unwrap();
    assert_eq!(of(&a, "SELECT T.D, W.D FROM T LEFT JOIN W PARTITION BY (P) ON T.D = W.D"), ["1,1", "2,2", "3,NULL"], "pad space");
    a.run("CREATE TABLE R (V INTEGER ARRAY[2], D INTEGER)").unwrap();
    a.run("INSERT INTO R VALUES (ARRAY[1, 2], 1), (ARRAY[1, 2], 2), (ARRAY[3], 3)").unwrap();
    let arrays = "SELECT R.V, T.D, R.D FROM T LEFT JOIN R PARTITION BY (V) ON T.D = R.D";
    assert_eq!(
        of(&a, arrays),
        ["ARRAY[1, 2],1,1", "ARRAY[1, 2],2,2", "ARRAY[1, 2],3,NULL", "ARRAY[3],1,NULL", "ARRAY[3],2,NULL", "ARRAY[3],3,3"]
    );
    let two = "SELECT S.P, S.AMOUNT, T.D FROM T LEFT JOIN S PARTITION BY (P, AMOUNT) ON T.D = S.D WHERE S.D IS NULL";
    assert_eq!(of(&a, two).len(), 8, "four partitions, each of one row, unmatched by two dates");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.10 SR9a-SR9d, SR10d, F]
/// a partitioned join table is refused (SQLCODE -103) as an operand of an
/// inner join [SR9a]; as the first operand of a LEFT join [SR9c]; as the
/// second of a RIGHT join, or one whose collection derived table reads
/// the first [SR9d]; if a column it names is of the other operand, an
/// outer reference, or no column of it [SR9b]; or if a corresponding join
/// column is a partitioning column [SR10d]. As the operand of a CROSS
/// join or of no join it is no SQL (SQLCODE -102). PARTITION, which
/// SQL-92 does not reserve, is still a correlation name.
#[test]
fn partitioned_joins_are_refused_7_10() {
    let (_db, a) = sales();
    for (refused, rule) in [
        ("SELECT * FROM T INNER JOIN S PARTITION BY (P) ON T.D = S.D", "SR9a"),
        ("SELECT * FROM T JOIN S PARTITION BY (P) USING (D)", "SR9a"),
        ("SELECT * FROM S PARTITION BY (P) LEFT JOIN T ON T.D = S.D", "SR9c"),
        ("SELECT * FROM T RIGHT JOIN S PARTITION BY (P) ON T.D = S.D", "SR9d"),
        ("SELECT * FROM T LEFT JOIN UNNEST(ARRAY[T.D]) AS X(N) PARTITION BY (N) ON T.D = X.N", "SR9d"),
        ("SELECT * FROM T LEFT JOIN S PARTITION BY (T.D) ON T.D = S.D", "SR9b"),
        ("SELECT * FROM T LEFT JOIN S PARTITION BY (Z) ON T.D = S.D", "SR9b"),
        ("SELECT (SELECT COUNT(*) FROM T LEFT JOIN S PARTITION BY (U.Q) ON T.D = S.D) FROM U", "SR9b"),
        ("SELECT * FROM T LEFT JOIN S PARTITION BY (D) USING (D)", "SR10d"),
        ("SELECT * FROM T NATURAL LEFT JOIN S PARTITION BY (D)", "SR10d"),
    ] {
        let e = fetched(&a, refused).expect_err(refused);
        assert_eq!(e.code, Code::SyntaxRule, "{refused}");
        let cited = format!("{}{}", e.msg, e.rule.as_deref().unwrap_or_default());
        assert!(cited.contains(&format!("§7.10 {rule}")), "{refused}: {cited}");
    }
    for syntax in ["SELECT * FROM T CROSS JOIN S PARTITION BY (P)", "SELECT * FROM T PARTITION BY (D)"] {
        assert_eq!(rows(&a, syntax).err(), Some(Code::Syntax), "{syntax}");
    }
    assert_eq!(of(&a, "SELECT PARTITION.D FROM T PARTITION"), ["1", "2", "3"]);
    let named = "SELECT COUNT(*) FROM T PARTITION LEFT JOIN S PARTITION BY (P) ON PARTITION.D = S.D";
    assert_eq!(rows(&a, named).unwrap(), ["9"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.10 F, §7.6] a
/// partitioned operand may be a parenthesized joined table, partitioned by
/// a column of any table in it, or a collection derived table that reads
/// nothing before it. A view of a partitioned join gives its rows, and a
/// prepared statement's parameter in its condition is the value given.
#[test]
fn partitioned_operands_of_any_table_primary_7_10() {
    let (_db, a) = sales();
    let parenthesized = "SELECT * FROM T LEFT JOIN (S JOIN U ON S.P = U.P) PARTITION BY (U.Q) ON T.D = S.D";
    assert_eq!(
        of(&a, parenthesized),
        ["1,NULL,NULL,NULL,NULL,8", "1,a,1,10,a,7", "2,NULL,NULL,NULL,NULL,7", "2,b,2,20,b,8", "3,NULL,NULL,NULL,NULL,8", "3,a,3,30,a,7"]
    );
    let unnested = "SELECT * FROM T LEFT JOIN UNNEST(ARRAY[1, 2]) AS X(N) PARTITION BY (N) ON T.D = X.N";
    assert_eq!(of(&a, unnested), ["1,1", "1,2", "2,1", "2,2", "3,1", "3,2"]);
    a.run("CREATE VIEW V AS SELECT S.P, T.D, S.AMOUNT FROM T LEFT JOIN S PARTITION BY (P) ON T.D = S.D").unwrap();
    assert_eq!(of(&a, "SELECT * FROM V WHERE AMOUNT IS NULL").len(), 5);
    let p = a.prepare("SELECT S.P, T.D FROM T LEFT JOIN S PARTITION BY (P) ON T.D = S.D AND S.AMOUNT > ? WHERE S.AMOUNT IS NULL").unwrap();
    let mut cursor = a.open_prepared(&p, &[Value::Exact(Decimal::new(15, 0))]).unwrap();
    let mut count = 0;
    while cursor.fetch().unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 7, "of 9, the sales of 20 and 30 matched");
    a.rollback().unwrap();
}

/// Schema A: T, of 100 dates; S, of 3000 sales of 10 products or none,
/// each with a note that makes it wide, of either store.
fn many(store: &str) -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (D INTEGER) {store}
           CREATE TABLE S (P INTEGER, D INTEGER, AMOUNT INTEGER, NOTE VARCHAR(40)) {store}"
    ))
    .unwrap();
    let a = db.session("A");
    let dates: Vec<String> = (0..100).map(|d| format!("({d})")).collect();
    a.run(&format!("INSERT INTO T VALUES {}", dates.join(", "))).unwrap();
    for first in (0..3000).step_by(500) {
        let values: Vec<String> = (first..first + 500)
            .map(|k: i64| {
                let p = if k % 17 == 0 { "NULL".to_owned() } else { (k % 10).to_string() };
                format!("({p}, {}, {k}, 'note of sale {k} written out wide')", k * 7 % 130)
            })
            .collect();
        a.run(&format!("INSERT INTO S VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The partitioned LEFT join of T and S on `on`, and what GR4 and GR5b
/// say it is, written without an outer join: the inner join, and each row
/// of T with each partition of S no row of that partition matches, its
/// partitioning column the partition's [GR4h].
fn left_and_meant(on: &str) -> (String, String) {
    let joined = format!("SELECT T.D, S.P, S.AMOUNT FROM T LEFT JOIN S PARTITION BY (P) ON {on}");
    let meant = format!(
        "SELECT T.D, S.P, S.AMOUNT FROM T JOIN S ON {on} UNION ALL \
         SELECT T.D, G.P, CAST(NULL AS INTEGER) FROM T, (SELECT DISTINCT P FROM S) AS G \
         WHERE NOT EXISTS (SELECT * FROM S WHERE S.P IS NOT DISTINCT FROM G.P AND {on})"
    );
    (joined, meant)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.10 GR4, GR5b] [DESIGN]
/// RULES.md D31, D38: a partitioned LEFT join of tables of either store,
/// by batches and row by row, gives what GR4 says, written as the inner
/// join and the rows XA without an outer join; with MEMORY.WORK_BYTES at
/// its least it spills and gives the same rows. EXPLAIN shows the outer
/// join hashed on the partitioning column IS NOT DISTINCT FROM its
/// partition's.
#[test]
fn partitioned_joins_answer_as_defined_7_10() {
    for store in ["", "USING columnstore"] {
        let (db, a) = many(store);
        let counted = Counted::default();
        db.set_spill(Box::new(counted.clone())).unwrap();
        let plan = a.explain(&left_and_meant("T.D = S.D").0, &Params::new()).unwrap();
        assert!(plan.contains("IS NOT DISTINCT FROM") && plan.contains("Distinct"), "{plan}");
        let cases: Vec<(String, String)> = ["T.D = S.D", "T.D = S.D AND S.AMOUNT > 1500"].iter().map(|on| left_and_meant(on)).collect();
        let expected: Vec<Vec<String>> = cases.iter().map(|(_, meant)| of(&a, meant)).collect();
        for ((joined, _), want) in cases.iter().zip(&expected) {
            let (batched, row) = both_ways(&a, joined);
            assert_eq!(&batched, want, "{joined} {store}");
            assert_eq!(&row, want, "{joined} {store}, row by row");
        }
        counted.taken();
        exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
        for ((joined, _), want) in cases.iter().zip(&expected) {
            let (batched, row) = both_ways(&a, joined);
            assert!(counted.taken() > 0, "{joined} {store} spilled");
            assert_eq!(&batched, want, "{joined} {store}, spilled");
            assert_eq!(&row, want, "{joined} {store}, spilled, row by row");
        }
    }
}
