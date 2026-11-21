//! Groupjoins (RULES.md D490): a grouping on a join's key computed in the
//! join's table, and the plans of decision-support queries shaped so.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Rows of `n` of `of`, as a VALUES list.
fn values(n: i64, of: &dyn Fn(i64) -> String) -> String {
    (0..n).map(of).collect::<Vec<_>>().join(", ")
}

/// Schema H: the tables of a decision-support benchmark, few rows each,
/// keyed as it keys them.
fn decision() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION H
           CREATE TABLE CUSTOMER (C_CUSTKEY INTEGER NOT NULL, C_NAME VARCHAR(25), C_NATIONKEY INTEGER, C_ACCTBAL DECIMAL(15,2),
             C_MKTSEGMENT CHAR(10), CONSTRAINT CUSTOMER_PK PRIMARY KEY (C_CUSTKEY))
           CREATE TABLE PART (P_PARTKEY INTEGER NOT NULL, P_BRAND CHAR(10), P_CONTAINER CHAR(10),
             CONSTRAINT PART_PK PRIMARY KEY (P_PARTKEY))
           CREATE TABLE ORDERS (O_ORDERKEY BIGINT NOT NULL, O_CUSTKEY INTEGER NOT NULL, O_TOTALPRICE DECIMAL(15,2),
             O_ORDERDATE DATE, O_SHIPPRIORITY INTEGER, O_COMMENT VARCHAR(79), CONSTRAINT ORDERS_PK PRIMARY KEY (O_ORDERKEY))
           CREATE TABLE LINEITEM (L_ORDERKEY BIGINT NOT NULL, L_PARTKEY INTEGER NOT NULL, L_LINENUMBER INTEGER NOT NULL,
             L_QUANTITY DECIMAL(15,2) NOT NULL, L_EXTENDEDPRICE DECIMAL(15,2) NOT NULL, L_DISCOUNT DECIMAL(15,2) NOT NULL,
             L_SHIPDATE DATE, CONSTRAINT LINEITEM_PK PRIMARY KEY (L_ORDERKEY, L_LINENUMBER))",
    )
    .unwrap();
    let s = db.session("H");
    let segment = |k: i64| ["BUILDING", "MACHINERY", "AUTOMOBILE"][usize::try_from(k % 3).unwrap()];
    s.run(&format!(
        "INSERT INTO CUSTOMER VALUES {}",
        values(150, &|k| format!("({k}, 'Customer#{k}', {}, {}.{:02}, '{}')", k % 25, k * 37 % 9999, k % 100, segment(k)))
    ))
    .unwrap();
    s.run(&format!(
        "INSERT INTO PART VALUES {}",
        values(200, &|k| format!("({k}, 'Brand#{}', '{}')", 21 + k % 5, if k % 4 == 0 { "MED BOX" } else { "SM CASE" }))
    ))
    .unwrap();
    let date = |k: i64| format!("DATE '199{}-{:02}-{:02}'", 2 + k % 6, 1 + k % 12, 1 + k % 28);
    s.run(&format!(
        "INSERT INTO ORDERS VALUES {}",
        values(1500, &|k| {
            let note = if k % 9 == 0 { "special packages requests" } else { "regular" };
            // Customers of a key divisible by 3 order nothing, as a third do.
            let customer = k * 7 % 150 - k * 7 % 150 % 3 + 1;
            format!("({k}, {customer}, {}.{:02}, {}, {}, '{note}')", k * 131 % 50000, k % 100, date(k), k % 2)
        })
    ))
    .unwrap();
    s.run(&format!(
        "INSERT INTO LINEITEM VALUES {}",
        values(6000, &|k| {
            let (order, line) = (k.div_euclid(4), k % 4);
            format!(
                "({order}, {}, {line}, {}, {}.{:02}, 0.0{}, {})",
                k * 13 % 200,
                1 + k * 7 % 50,
                k * 17 % 9000,
                k % 100,
                k % 9,
                date(k.div_euclid(3))
            )
        })
    ))
    .unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// The physical plan EXPLAIN shows of `sql`.
fn physical(s: &Session, sql: &str) -> String {
    let explained = s.explain(sql, &Params::new()).unwrap();
    explained.split("-- physical:").last().unwrap_or_default().to_owned()
}

const Q3: &str = "SELECT L_ORDERKEY, SUM(L_EXTENDEDPRICE * (1 - L_DISCOUNT)) AS REVENUE, O_ORDERDATE, O_SHIPPRIORITY
  FROM CUSTOMER, ORDERS, LINEITEM
  WHERE C_MKTSEGMENT = 'BUILDING' AND C_CUSTKEY = O_CUSTKEY AND L_ORDERKEY = O_ORDERKEY
    AND O_ORDERDATE < DATE '1995-03-15' AND L_SHIPDATE > DATE '1995-03-15'
  GROUP BY L_ORDERKEY, O_ORDERDATE, O_SHIPPRIORITY
  ORDER BY REVENUE DESC, O_ORDERDATE FETCH FIRST 10 ROWS ONLY";

const Q13: &str = "SELECT C_COUNT, COUNT(*) AS CUSTDIST
  FROM (SELECT C_CUSTKEY, COUNT(O_ORDERKEY) FROM CUSTOMER LEFT OUTER JOIN ORDERS
          ON C_CUSTKEY = O_CUSTKEY AND O_COMMENT NOT LIKE '%special%requests%' GROUP BY C_CUSTKEY) AS C_ORDERS (C_CUSTKEY, C_COUNT)
  GROUP BY C_COUNT ORDER BY CUSTDIST DESC, C_COUNT DESC";

const Q17: &str = "SELECT SUM(L_EXTENDEDPRICE) / 7.0 AS AVG_YEARLY FROM LINEITEM, PART
  WHERE P_PARTKEY = L_PARTKEY AND P_BRAND = 'Brand#23' AND P_CONTAINER = 'MED BOX'
    AND L_QUANTITY < (SELECT 0.2 * AVG(L_QUANTITY) FROM LINEITEM WHERE L_PARTKEY = P_PARTKEY)";

const Q18: &str = "SELECT C_NAME, C_CUSTKEY, O_ORDERKEY, O_ORDERDATE, O_TOTALPRICE, SUM(L_QUANTITY)
  FROM CUSTOMER, ORDERS, LINEITEM
  WHERE O_ORDERKEY IN (SELECT L_ORDERKEY FROM LINEITEM GROUP BY L_ORDERKEY HAVING SUM(L_QUANTITY) > 100)
    AND C_CUSTKEY = O_CUSTKEY AND O_ORDERKEY = L_ORDERKEY
  GROUP BY C_NAME, C_CUSTKEY, O_ORDERKEY, O_ORDERDATE, O_TOTALPRICE
  ORDER BY O_TOTALPRICE DESC, O_ORDERDATE FETCH FIRST 100 ROWS ONLY";

const Q10: &str = "SELECT C_CUSTKEY, C_NAME, SUM(L_EXTENDEDPRICE * (1 - L_DISCOUNT)) AS REVENUE, C_ACCTBAL
  FROM CUSTOMER, ORDERS, LINEITEM
  WHERE C_CUSTKEY = O_CUSTKEY AND L_ORDERKEY = O_ORDERKEY AND O_ORDERDATE >= DATE '1993-10-01'
  GROUP BY C_CUSTKEY, C_NAME, C_ACCTBAL ORDER BY REVENUE DESC FETCH FIRST 20 ROWS ONLY";

/// Groupings of joins on their keys in no order, so their groups' order is
/// the plan's: by the group operand's rows, or by their first joined rows.
const UNORDERED: [&str; 4] = [
    "SELECT O_ORDERKEY, COUNT(*), SUM(L_QUANTITY), MIN(L_SHIPDATE) FROM ORDERS JOIN LINEITEM ON O_ORDERKEY = L_ORDERKEY GROUP BY O_ORDERKEY",
    "SELECT L_ORDERKEY, O_ORDERDATE, SUM(L_EXTENDEDPRICE) FROM LINEITEM JOIN ORDERS ON O_ORDERKEY = L_ORDERKEY GROUP BY L_ORDERKEY, O_ORDERDATE",
    "SELECT C_CUSTKEY, COUNT(O_ORDERKEY), COUNT(*), AVG(O_TOTALPRICE) FROM CUSTOMER LEFT JOIN ORDERS ON C_CUSTKEY = O_CUSTKEY GROUP BY C_CUSTKEY",
    "SELECT C_CUSTKEY, C_NAME, MAX(O_TOTALPRICE) FROM CUSTOMER JOIN ORDERS ON C_CUSTKEY = O_CUSTKEY GROUP BY C_CUSTKEY, C_NAME",
];

/// The rows of `sql` in the order they come, groupjoins on and off, by
/// batches and row by row: all four alike.
fn alike(s: &Session, sql: &str) -> Vec<String> {
    let on = rows(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let rows_on = rows(s, sql);
    exec(s, "SET OPTIMIZER.GROUPJOIN = 'OFF'").unwrap();
    let rows_off = rows(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    let off = rows(s, sql);
    exec(s, "SET OPTIMIZER.GROUPJOIN = 'ON'").unwrap();
    assert_eq!(on, off, "{sql}");
    assert_eq!(rows_on, rows_off, "{sql}");
    assert_eq!(on, rows_on, "{sql}");
    on.unwrap_or_default()
}

/// Runs `s`'s parts in parallel on four threads, of small morsels.
fn parallel(s: &Session) {
    for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
        exec(s, set).unwrap();
    }
}

/// A database of schema H whose statements may run on four threads.
fn decision_on_four() -> Database {
    let db = decision();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db
}

/// Verifies: [DESIGN] (RULES.md D490) the decision-support queries shaped
/// as groupjoins, and groupings of joins in no order, give the same rows
/// in the same order with groupjoins on and off, by batches and row by
/// row; of a LEFT join, its unmatched groups as the join gives them.
#[test]
fn groupjoins_give_the_groupings_rows() {
    let db = decision();
    let s = db.session("H");
    for q in [Q3, Q10, Q13, Q17, Q18].into_iter().chain(UNORDERED) {
        alike(&s, q);
    }
    assert!(physical(&s, Q13).contains("LeftGroupJoin group by [CUSTOMER.C_CUSTKEY]"), "{}", physical(&s, Q13));
    assert!(physical(&s, Q17).contains("LeftGroupJoin"), "{}", physical(&s, Q17));
    for q in UNORDERED {
        assert!(physical(&s, q).contains("GroupJoin group by"), "{q}");
    }
    let lefts = alike(&s, UNORDERED[2]);
    assert_eq!(lefts.len(), 150, "every customer a group");
    assert!(lefts.iter().any(|r| r.contains(",0,1,")), "a customer of no order: {lefts:?}");
}

/// Verifies: [DESIGN] (RULES.md D490) EXPLAIN shows a groupjoin's grouping
/// and join, its build then its probe; OPTIMIZER.GROUPJOIN OFF leaves the
/// grouping over the join.
#[test]
fn explain_shows_a_groupjoin() {
    let db = decision();
    let s = db.session("H");
    let shown = physical(&s, UNORDERED[2]);
    let lines: Vec<&str> = shown.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    assert!(lines.get(1).is_some_and(|l| l.starts_with("LeftGroupJoin group by [CUSTOMER.C_CUSTKEY] computing [COUNT(ORDERS.O_ORDERKEY), COUNT(*), AVG(ORDERS.O_TOTALPRICE)] on CUSTOMER.C_CUSTKEY = ORDERS.O_CUSTKEY")), "{shown}");
    assert!(lines.get(2).is_some_and(|l| l.starts_with("SeqScan H.CUSTOMER")), "{shown}");
    assert!(lines.get(3).is_some_and(|l| l.starts_with("SeqScan H.ORDERS")), "{shown}");
    exec(&s, "SET OPTIMIZER.GROUPJOIN = 'OFF'").unwrap();
    let off = physical(&s, UNORDERED[2]);
    assert!(!off.contains("GroupJoin") && off.contains("Aggregate group by") && off.contains("LeftOuterJoin"), "{off}");
}

/// Groupings of joins no groupjoin computes: a group of many build rows,
/// a set function DISTINCT or reading the group operand, no grouping
/// column, a LEFT join's right operand as the group operand, keys of
/// character strings.
const NOT_GROUPJOINS: [&str; 6] = [
    "SELECT O_CUSTKEY, COUNT(*) FROM ORDERS JOIN LINEITEM ON O_ORDERKEY = L_ORDERKEY GROUP BY O_CUSTKEY",
    "SELECT O_ORDERKEY, COUNT(DISTINCT L_PARTKEY) FROM ORDERS JOIN LINEITEM ON O_ORDERKEY = L_ORDERKEY GROUP BY O_ORDERKEY",
    "SELECT O_ORDERKEY, SUM(O_TOTALPRICE + L_QUANTITY) FROM ORDERS JOIN LINEITEM ON O_ORDERKEY = L_ORDERKEY GROUP BY O_ORDERKEY",
    "SELECT COUNT(*), SUM(L_QUANTITY) FROM ORDERS JOIN LINEITEM ON O_ORDERKEY = L_ORDERKEY",
    "SELECT O_ORDERKEY, COUNT(*) FROM LINEITEM LEFT JOIN ORDERS ON O_ORDERKEY = L_ORDERKEY GROUP BY O_ORDERKEY",
    "SELECT CUSTOMER.C_NAME, COUNT(*) FROM CUSTOMER JOIN CUSTOMER AS D ON CUSTOMER.C_NAME = D.C_NAME GROUP BY CUSTOMER.C_NAME",
];

/// Verifies: [DESIGN] (RULES.md D490) each condition of a groupjoin
/// decides alone: a grouping that fails one is left over its join, its
/// rows the same.
#[test]
fn groupings_no_groupjoin_computes() {
    let db = decision();
    let s = db.session("H");
    for q in NOT_GROUPJOINS {
        let shown = physical(&s, q);
        assert!(
            !shown.contains("GroupJoin group by [ORDERS.O_CUSTKEY]") && !shown.contains("GroupJoin group by [CUSTOMER.C_NAME]"),
            "{q}\n{shown}"
        );
        alike(&s, q);
    }
    for q in &NOT_GROUPJOINS[1..5] {
        assert!(!physical(&s, q).lines().any(|l| l.contains("GroupJoin") && l.contains("ORDERS.O_ORDERKEY]")), "{q}");
    }
}

/// Verifies: [DESIGN] (RULES.md D490, D290) a groupjoin run in parallel
/// gives the rows, in order, that it gives alone and that the grouping
/// over its join gives, its probe's morsels grouped on four threads.
#[test]
fn groupjoins_in_parallel_give_the_rows_alone() {
    let db = decision_on_four();
    let s = db.session("H");
    let alone: Vec<_> = UNORDERED.iter().map(|q| alike(&s, q)).collect();
    parallel(&s);
    for (q, alone) in UNORDERED.iter().zip(alone) {
        let shown = physical(&s, q);
        assert!(shown.contains("Parallel") && shown.contains("GroupJoin"), "{shown}");
        assert_eq!(alike(&s, q), alone, "{q}");
    }
    alike(&s, Q13);
    alike(&s, Q17);
}

/// Schema N: a group operand keyed UNIQUE, its keys null in some rows, and
/// another keyed PRIMARY KEY, with a probe whose keys are null in some.
fn nulls() -> Database {
    let db = decision_on_four();
    let s = db.session("H");
    s.run("CREATE TABLE U (K INTEGER, V INTEGER, CONSTRAINT U_K UNIQUE (K))").unwrap();
    s.run("CREATE TABLE P (K INTEGER NOT NULL, V INTEGER, CONSTRAINT P_K PRIMARY KEY (K))").unwrap();
    s.run("CREATE TABLE R (K INTEGER, X DOUBLE PRECISION, B BIGINT)").unwrap();
    s.run(&format!("INSERT INTO U VALUES {}, (NULL, 1), (NULL, 2)", values(40, &|k| format!("({k}, {})", k % 7)))).unwrap();
    s.run(&format!("INSERT INTO P VALUES {}", values(40, &|k| format!("({k}, {})", k % 7)))).unwrap();
    let x = |k: i64| format!("{}e{}", k % 13 + 1, (k * 7) % 40 - 20);
    s.run(&format!(
        "INSERT INTO R VALUES {}",
        values(3000, &|k| if k % 11 == 0 { format!("(NULL, {}, {k})", x(k)) } else { format!("({}, {}, {k})", k % 50, x(k)) })
    ))
    .unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// Verifies: [X3.135-1986 §5.11 GR2] [ISO/IEC 9075:1992 §7.5 GR1d]
/// (RULES.md D490) a key null in a row of either operand joins nothing,
/// so the row is no group of an inner join and feeds none; a LEFT join's
/// row of a null key is a group fed one row of nulls; a group operand
/// UNIQUE but of null keys is no LEFT join's, whose grouping makes one
/// group of its nulls.
#[test]
fn nulls_in_keys_join_nothing() {
    let db = nulls();
    let s = db.session("H");
    let inner = "SELECT U.K, COUNT(*), SUM(R.X) FROM U JOIN R ON U.K = R.K GROUP BY U.K";
    let left = "SELECT P.K, COUNT(*), COUNT(R.K), SUM(R.X) FROM P LEFT JOIN R ON P.K = R.K GROUP BY P.K";
    let unique_left = "SELECT U.K, COUNT(*), COUNT(R.X) FROM U LEFT JOIN R ON U.K = R.K GROUP BY U.K";
    for q in [inner, left, unique_left] {
        alike(&s, q);
    }
    assert!(physical(&s, inner).contains("GroupJoin group by [U.K]"), "{}", physical(&s, inner));
    assert!(physical(&s, left).contains("LeftGroupJoin group by [P.K]"), "{}", physical(&s, left));
    assert!(!physical(&s, unique_left).contains("GroupJoin"), "{}", physical(&s, unique_left));
    assert_eq!(alike(&s, unique_left).iter().filter(|r| r.starts_with("NULL")).count(), 1, "one group of the nulls");
    parallel(&s);
    for q in [inner, left] {
        alike(&s, q);
    }
}

/// Verifies: [ISO/IEC 9075:1992 §6.5 GR1] (RULES.md D490) a sum of
/// approximate numbers of many magnitudes is the same sum, its values
/// taken in the order the join gives them, alone and in parallel; and a
/// sum past the largest exact number raises SQLCODE -900 as the grouping
/// over the join raises it.
#[test]
fn float_sums_and_overflow_as_the_join_gives_them() {
    let db = nulls();
    let s = db.session("H");
    let sums = "SELECT P.K, SUM(R.X), AVG(R.X) FROM R JOIN P ON P.K = R.K GROUP BY P.K";
    let alone = alike(&s, sums);
    assert!(physical(&s, sums).contains("GroupJoin"), "{}", physical(&s, sums));
    parallel(&s);
    assert_eq!(alike(&s, sums), alone);
    exec(&s, "SET EXECUTOR.DEGREE = 1").unwrap();
    let big = values(200, &|_| "(3, 1, 999999999999999999)".to_owned());
    s.run(&format!("INSERT INTO R VALUES {big}")).unwrap();
    let past = "SELECT P.K, SUM(R.B * R.B) FROM P LEFT JOIN R ON P.K = R.K GROUP BY P.K";
    let on = rows(&s, past);
    exec(&s, "SET OPTIMIZER.GROUPJOIN = 'OFF'").unwrap();
    let off = rows(&s, past);
    exec(&s, "SET OPTIMIZER.GROUPJOIN = 'ON'").unwrap();
    assert!(on.is_err(), "{on:?}");
    assert_eq!(on, off);
}

/// Verifies: [DESIGN] (RULES.md D490) a group operand that joins no row,
/// or no rows to join with, gives the grouping's rows: none of an inner
/// join, a row of nulls fed to each group of a LEFT one.
#[test]
fn empty_operands_give_the_groupings_rows() {
    let db = nulls();
    let s = db.session("H");
    for q in [
        "SELECT P.K, COUNT(*), SUM(R.X) FROM P JOIN R ON P.K = R.K WHERE R.K > 1000 GROUP BY P.K",
        "SELECT P.K, COUNT(*), SUM(R.X) FROM P LEFT JOIN (SELECT * FROM R WHERE K > 1000) AS E ON P.K = E.K GROUP BY P.K",
        "SELECT P.K, COUNT(*) FROM P LEFT JOIN R ON P.K = R.K WHERE P.K > 1000 GROUP BY P.K",
    ] {
        alike(&s, q);
    }
}

/// Verifies: [DESIGN] (RULES.md D490) a group operand of many rows beside
/// the few keys of the other's, each of many rows, is joined with the
/// other grouped first,
/// the eager form, its groups in the group operand's order or in the
/// order of their first rows as the join's operands say.
#[test]
fn eager_form_groups_the_probe_first() {
    let db = decision();
    let s = db.session("H");
    s.run("CREATE TABLE BIG (K INTEGER NOT NULL, V INTEGER, CONSTRAINT BIG_K PRIMARY KEY (K))").unwrap();
    s.run("CREATE TABLE FEW (K INTEGER NOT NULL, N INTEGER NOT NULL)").unwrap();
    s.run(&format!("INSERT INTO BIG VALUES {}", values(4000, &|k| format!("({k}, {})", k % 9)))).unwrap();
    s.run(&format!("INSERT INTO FEW VALUES {}", values(1000, &|k| format!("({}, {k})", (k * 7) % 12 * 100)))).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    let left = "SELECT BIG.K, COUNT(*), SUM(FEW.N), MAX(FEW.N) FROM BIG JOIN FEW ON BIG.K = FEW.K GROUP BY BIG.K";
    let right = "SELECT BIG.K, COUNT(*), MIN(FEW.N), AVG(FEW.N) FROM FEW JOIN BIG ON BIG.K = FEW.K GROUP BY BIG.K";
    for q in [left, right] {
        assert!(physical(&s, q).contains("early, by the keys of"), "{}", physical(&s, q));
        assert_eq!(alike(&s, q).len(), 12, "{q}");
    }
}

/// Verifies: [DESIGN] (RULES.md D490, D38) a groupjoin whose build holds
/// more than MEMORY.WORK_BYTES gives way to the grouping over its join,
/// and one past the work area gives the grouping's rows: as a join past it
/// gives its rows in no set order, the groups are compared in none.
#[test]
fn a_groupjoin_past_its_work_area_gives_way() {
    let db = decision_on_four();
    let s = db.session("H");
    exec(&s, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    for q in UNORDERED {
        alike_unordered(&s, q);
    }
    parallel(&s);
    for q in UNORDERED {
        alike_unordered(&s, q);
    }
}

/// The rows of `sql` sorted, groupjoins on and off, by batches and row by
/// row: all four alike.
fn alike_unordered(s: &Session, sql: &str) {
    let sorted = |off: Option<&str>| {
        if let Some(set) = off {
            exec(s, set).unwrap();
        }
        let mut found = rows(s, sql).unwrap_or_default();
        found.sort();
        found
    };
    let on = sorted(None);
    let rows_on = sorted(Some("SET EXECUTOR.BATCHES = 'OFF'"));
    let rows_off = sorted(Some("SET OPTIMIZER.GROUPJOIN = 'OFF'"));
    let off = sorted(Some("SET EXECUTOR.BATCHES = 'ON'"));
    exec(s, "SET OPTIMIZER.GROUPJOIN = 'ON'").unwrap();
    assert!(!on.is_empty(), "{sql}");
    assert_eq!(on, off, "{sql}");
    assert_eq!(rows_on, rows_off, "{sql}");
    assert_eq!(on, rows_on, "{sql}");
}
