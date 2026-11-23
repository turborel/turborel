//! Plans of queries shaped as a decision-support and an order-entry
//! benchmark's: an equality each operand of an OR repeats joins by it
//! (RULES.md D35), tables listed apart are joined through the one between
//! them (D300), and an index whose longer prefix a query holds equal is
//! read over a shorter one (D302). These are `[DESIGN]` requirements on
//! the plan; the rows are the same either way [X3.135-1986 §5.18 GR2,
//! §5.20 GR2b].

use turborel::db::{Database, Session};
use turborel::optimizer::FACTOR_DISJUNCTIONS;
use turborel::params::Params;

use crate::common::{exec, set_rule, sorted};

/// The physical plan EXPLAIN shows of `sql`.
fn physical(s: &Session, sql: &str) -> String {
    let explained = s.explain(sql, &Params::new()).unwrap();
    explained.split("-- physical:").last().unwrap_or_default().to_owned()
}

/// Rows of `n` of `of`, as a VALUES list.
fn values(n: i32, of: &dyn Fn(i32) -> String) -> String {
    (0..n).map(of).collect::<Vec<_>>().join(", ")
}

/// Schema H: parts P of brands and sizes, line items L of quantities,
/// suppliers S, and the offers PS of a part by a supplier.
fn decision() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION H
           CREATE TABLE P (K INTEGER, B VARCHAR(10), SZ INTEGER)
           CREATE TABLE L (K INTEGER, Q INTEGER, M VARCHAR(10))
           CREATE TABLE S (K INTEGER, N INTEGER)
           CREATE TABLE PS (PK INTEGER, SK INTEGER, COST INTEGER)
           CREATE TABLE N (K INTEGER, R INTEGER)
           CREATE TABLE R (K INTEGER, NM VARCHAR(10))",
    )
    .unwrap();
    let s = db.session("H");
    s.run(&format!("INSERT INTO P VALUES {}", values(400, &|k| format!("({k}, 'B{}', {})", k % 5, k % 50)))).unwrap();
    s.run(&format!("INSERT INTO L VALUES {}", values(3000, &|k| format!("({}, {}, 'M{}')", (k * 7) % 400, k % 50, k % 3)))).unwrap();
    s.run(&format!("INSERT INTO S VALUES {}", values(300, &|k| format!("({k}, {})", k % 25)))).unwrap();
    s.run(&format!("INSERT INTO PS VALUES {}", values(1600, &|k| format!("({}, {}, {})", k % 400, (k * 13) % 300, k % 97)))).unwrap();
    s.run(&format!("INSERT INTO N VALUES {}", values(25, &|k| format!("({k}, {})", k % 5)))).unwrap();
    s.run(&format!("INSERT INTO R VALUES {}", values(5, &|k| format!("({k}, 'R{k}')")))).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// Verifies: [X3.135-1986 §5.18 GR2]; [DESIGN] RULES.md D35,
/// `FACTOR_DISJUNCTIONS`: the join's equality repeated in each operand of
/// an OR is taken out of it, so the two tables are hash joined, not
/// multiplied; each table is filtered first by the OR of its own
/// conjuncts; the rows are those of the plan without the rule, which
/// multiplies them.
#[test]
fn an_equality_each_disjunct_repeats_joins() {
    let db = decision();
    let s = db.session("H");
    let sql = "SELECT P.K, L.Q FROM P, L WHERE (P.K = L.K AND P.B = 'B1' AND L.Q < 10 AND L.M = 'M1') \
               OR (P.K = L.K AND P.B = 'B2' AND L.Q > 40) OR (L.K = P.K AND P.K = L.K AND P.B = 'B3' AND P.SZ < 5)";
    let plan = physical(&s, sql);
    assert!(plan.contains("HashJoin") && !plan.contains("Cartesian"), "{plan}");
    assert!(plan.contains("Filter ((P.B = 'B1' OR P.B = 'B2') OR P.B = 'B3' AND P.SZ < 5)"), "{plan}");
    assert!(!plan.contains("OR L.K = P.K"), "{plan}");
    let rows = sorted(&s, sql);
    assert_ne!(rows, Vec::<String>::new());
    set_rule(&db, FACTOR_DISJUNCTIONS, false);
    assert!(physical(&s, sql).contains("Cartesian"));
    assert_eq!(sorted(&s, sql), rows);
    set_rule(&db, FACTOR_DISJUNCTIONS, true);
    let absorbed = "SELECT P.K FROM P, L WHERE P.K = L.K OR (P.K = L.K AND P.B = 'B1')";
    assert!(!physical(&s, absorbed).contains("Cartesian"));
}

/// Verifies: [DESIGN] RULES.md D300: of tables listed parts, suppliers,
/// offers, nations and regions, the parts and the suppliers each joined
/// only to the offers, no plan multiplies two tables no condition joins,
/// as the order written would; the rows are those of the greedy order.
#[test]
fn tables_listed_apart_join_through_the_one_between() {
    let db = decision();
    let s = db.session("H");
    let three = "SELECT P.K, S.N, PS.COST FROM P, S, PS WHERE P.K = PS.PK AND S.K = PS.SK AND P.SZ = 15 AND S.N < 5";
    let five = "SELECT P.K, S.K, PS.COST FROM P, S, PS, N, R \
                WHERE P.K = PS.PK AND S.K = PS.SK AND P.SZ = 15 AND S.N = N.K AND N.R = R.K AND R.NM = 'R1'";
    for sql in [three, five] {
        let plan = physical(&s, sql);
        assert!(!plan.contains("Cartesian") && plan.contains("join order by DPccp"), "{plan}");
        let rows = sorted(&s, sql);
        exec(&s, "SET PLANNER.JOIN_ENUMERATION = 'GREEDY'").unwrap();
        assert_eq!(sorted(&s, sql), rows);
        exec(&s, "SET PLANNER.JOIN_ENUMERATION = 'ADAPTIVE'").unwrap();
    }
}

/// Schema C: customers keyed by warehouse, district and id, with an index
/// of those and one on warehouse, district, last name and id; 20
/// warehouses of one district each, numbered as the warehouse, so the
/// two columns go together; 1,100 customers a district, two of each last
/// name.
fn customers() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION C
           CREATE TABLE CU (W NUMERIC(4), D NUMERIC(4), ID NUMERIC(6), LNAME VARCHAR(16), BAL INTEGER, PRIMARY KEY (W, D, ID))",
    )
    .unwrap();
    let s = db.session("C");
    for w in 1..=20 {
        let rows = values(1100, &|id| format!("({w}, {w}, {id}, 'N{}', {})", id % 550, id % 1000));
        s.run(&format!("INSERT INTO CU VALUES {rows}")).unwrap();
    }
    s.commit().unwrap();
    db.create_index("CU_KEY", "C.CU", &["W", "D", "ID"], "btree").unwrap();
    db.create_index("CU_NAME", "C.CU", &["W", "D", "LNAME", "ID"], "btree").unwrap();
    // A table keeps zones from the first scan told a condition of a column
    // kept (RULES.md D868): scanned once by a column no index reads, then
    // its zones summed up.
    assert_eq!(sorted(&s, "SELECT ID FROM CU WHERE BAL < 0").len(), 0);
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// The rows EXPLAIN estimates the read of `plan` finds: the number after
/// its `est. rows=` or `est. rows each=`.
fn estimated(plan: &str) -> f64 {
    let (_, after) = plan.split_once("est. rows").unwrap_or_default();
    let digits: String = after.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
    digits.parse().unwrap_or(0.0)
}

/// Verifies: [DESIGN] RULES.md D302, D381, D839: of a customer found by
/// warehouse, district and last name, the index whose prefix holds all
/// three is read, not the key's prefix of two. That prefix, of 1,100 rows,
/// past what the key counts, is estimated by the share of the tree between
/// its bounds, within twice its rows, not the 55 a guess of independent
/// columns gives. Its rows lie together in the zones a scan of the district
/// reads, so the key's range, priced as that scan, is read (D839).
#[test]
fn a_longer_equal_prefix_is_read() {
    let db = customers();
    let s = db.session("C");
    let sql = "SELECT ID, BAL FROM CU WHERE W = 1 AND D = 1 AND LNAME = 'N7'";
    let plan = physical(&s, sql);
    assert!(plan.contains("using CU_NAME"), "{plan}");
    assert_eq!(sorted(&s, sql).len(), 2);
    let district = physical(&s, "SELECT ID, BAL FROM CU WHERE W = 1 AND D = 1");
    let est = estimated(&district);
    assert!(district.contains("IndexScan C.CU") && !district.contains("CU_NAME"), "{district}");
    assert!((550.0..=2200.0).contains(&est), "{district}");
    assert_eq!(sorted(&s, "SELECT ID FROM CU WHERE W = 1 AND D = 1").len(), 1100);
}
