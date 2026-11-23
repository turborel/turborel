//! \[DESIGN\] The TPC-C transactions, as procedures CALLed through prepared
//! statements, keep every plan once they have run a while (RULES.md D613):
//! a CALL in steady state plans nothing, its statements' plans kept as the
//! tables they read grow (`physical::plannings`), its SELECT ... INTO of a
//! change's delta table among them (D793).

use std::sync::Arc;

use turborel::binder::bindings;
use turborel::db::{Database, Prepared, Session};
use turborel::physical::plannings;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::rows;

/// The procedures, one after another.
const PROCEDURES: &str = include_str!("tpcc_plans/procedures.sql");

/// The tables and indexes, as the driver makes them.
const TABLES: [&str; 11] = [
    "CREATE TABLE WAREHOUSE (W_ID INTEGER NOT NULL, W_YTD DECIMAL(12,2), W_TAX DECIMAL(4,4), W_NAME VARCHAR(10), W_STREET_1 VARCHAR(20), W_STREET_2 VARCHAR(20), W_CITY VARCHAR(20), W_STATE CHAR(2), W_ZIP CHAR(9), CONSTRAINT WAREHOUSE_I1 PRIMARY KEY (W_ID))",
    "CREATE TABLE DISTRICT (D_W_ID INTEGER NOT NULL, D_ID SMALLINT NOT NULL, D_YTD DECIMAL(12,2), D_TAX DECIMAL(4,4), D_NEXT_O_ID INTEGER, D_NAME VARCHAR(10), D_STREET_1 VARCHAR(20), D_STREET_2 VARCHAR(20), D_CITY VARCHAR(20), D_STATE CHAR(2), D_ZIP CHAR(9), CONSTRAINT DISTRICT_I1 PRIMARY KEY (D_W_ID, D_ID))",
    "CREATE TABLE CUSTOMER (C_W_ID INTEGER NOT NULL, C_D_ID SMALLINT NOT NULL, C_ID INTEGER NOT NULL, C_DISCOUNT DECIMAL(4,4), C_CREDIT CHAR(2), C_LAST VARCHAR(16), C_FIRST VARCHAR(16), C_CREDIT_LIM DECIMAL(12,2), C_BALANCE DECIMAL(12,2), C_YTD_PAYMENT DECIMAL(12,2), C_PAYMENT_CNT SMALLINT, C_DELIVERY_CNT SMALLINT, C_STREET_1 VARCHAR(20), C_STREET_2 VARCHAR(20), C_CITY VARCHAR(20), C_STATE CHAR(2), C_ZIP CHAR(9), C_PHONE CHAR(16), C_SINCE TIMESTAMP(0), C_MIDDLE CHAR(2), C_DATA VARCHAR(500), CONSTRAINT CUSTOMER_I1 PRIMARY KEY (C_W_ID, C_D_ID, C_ID))",
    "CREATE TABLE HISTORY (H_C_ID INTEGER, H_C_D_ID SMALLINT, H_C_W_ID INTEGER, H_D_ID SMALLINT, H_W_ID INTEGER, H_DATE TIMESTAMP(0), H_AMOUNT DECIMAL(6,2), H_DATA VARCHAR(24))",
    "CREATE TABLE NEW_ORDER (NO_W_ID INTEGER NOT NULL, NO_D_ID SMALLINT NOT NULL, NO_O_ID INTEGER NOT NULL, CONSTRAINT NEW_ORDER_I1 PRIMARY KEY (NO_W_ID, NO_D_ID, NO_O_ID))",
    "CREATE TABLE ORDERS (O_W_ID INTEGER NOT NULL, O_D_ID SMALLINT NOT NULL, O_ID INTEGER NOT NULL, O_C_ID INTEGER, O_CARRIER_ID SMALLINT, O_OL_CNT SMALLINT, O_ALL_LOCAL SMALLINT, O_ENTRY_D TIMESTAMP(0), CONSTRAINT ORDERS_I1 PRIMARY KEY (O_W_ID, O_D_ID, O_ID))",
    "CREATE TABLE ORDER_LINE (OL_W_ID INTEGER NOT NULL, OL_D_ID SMALLINT NOT NULL, OL_O_ID INTEGER NOT NULL, OL_NUMBER SMALLINT NOT NULL, OL_I_ID INTEGER, OL_DELIVERY_D TIMESTAMP(0), OL_AMOUNT DECIMAL(6,2), OL_SUPPLY_W_ID INTEGER, OL_QUANTITY SMALLINT, OL_DIST_INFO CHAR(24), CONSTRAINT ORDER_LINE_I1 PRIMARY KEY (OL_W_ID, OL_D_ID, OL_O_ID, OL_NUMBER))",
    "CREATE TABLE ITEM (I_ID INTEGER NOT NULL, I_IM_ID INTEGER, I_NAME VARCHAR(24), I_PRICE DECIMAL(5,2), I_DATA VARCHAR(50), CONSTRAINT ITEM_I1 PRIMARY KEY (I_ID))",
    "CREATE TABLE STOCK (S_W_ID INTEGER NOT NULL, S_I_ID INTEGER NOT NULL, S_QUANTITY SMALLINT, S_DIST_01 CHAR(24), S_DIST_02 CHAR(24), S_DIST_03 CHAR(24), S_DIST_04 CHAR(24), S_DIST_05 CHAR(24), S_DIST_06 CHAR(24), S_DIST_07 CHAR(24), S_DIST_08 CHAR(24), S_DIST_09 CHAR(24), S_DIST_10 CHAR(24), S_YTD INTEGER, S_ORDER_CNT SMALLINT, S_REMOTE_CNT SMALLINT, S_DATA VARCHAR(50), CONSTRAINT STOCK_I1 PRIMARY KEY (S_W_ID, S_I_ID))",
    "CREATE INDEX CUSTOMER_I2 ON CUSTOMER (C_W_ID, C_D_ID, C_LAST, C_FIRST, C_ID)",
    "CREATE INDEX ORDERS_I2 ON ORDERS (O_W_ID, O_D_ID, O_C_ID, O_ID)",
];

/// Items, customers in each district and orders in each district loaded:
/// enough items and orders that ORDER_LINE and STOCK are far larger than
/// the lines of a district's last twenty orders and their stock. Reading
/// those through an index then costs far less than reading either table
/// whole, so no plan is near another in cost as the tables grow.
const ITEMS: i64 = 20_000;
const CUSTOMERS: i64 = 30;
const ORDERS: i64 = 400;

/// A small pseudo-random sequence, so each run CALLs with the same values.
struct Sequence(u64);

impl Sequence {
    /// A value from `low` to `high`.
    fn between(&mut self, low: i64, high: i64) -> i64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let span = u64::try_from(high.wrapping_sub(low).wrapping_add(1)).unwrap();
        low.wrapping_add(i64::try_from((self.0 >> 33) % span).unwrap())
    }
}

/// A customer's last name, as the specification makes it of `n`.
fn last_name(n: i64) -> String {
    const SYLLABLES: [&str; 10] = ["BAR", "OUGHT", "ABLE", "PRI", "PRES", "ESE", "ANTI", "CALLY", "ATION", "EING"];
    let at = |k: i64| SYLLABLES[usize::try_from(k).unwrap()];
    format!("{}{}{}", at(n.div_euclid(100)), at(n.div_euclid(10) % 10), at(n % 10))
}

/// `rows` inserted into `table` by statements of at most 200 rows each.
fn load(s: &Session, table: &str, rows: &[String]) {
    for part in rows.chunks(200) {
        s.run(&format!("INSERT INTO {table} VALUES {}", part.join(", "))).unwrap();
    }
}

/// The larger tables, each made by one INSERT of a query over N, the
/// numbers 1 to `ITEMS`: ITEM, STOCK, ORDERS, ORDER_LINE and NEW_ORDER.
fn generated(s: &Session) {
    s.run("CREATE TABLE DIGIT (D INTEGER NOT NULL)").unwrap();
    s.run("INSERT INTO DIGIT VALUES (0), (1), (2), (3), (4), (5), (6), (7), (8), (9)").unwrap();
    s.run("CREATE TABLE N (N INTEGER NOT NULL)").unwrap();
    s.run(&format!(
        "INSERT INTO N SELECT * FROM (SELECT A.D * 10000 + B.D * 1000 + C.D * 100 + E.D * 10 + F.D + 1 AS N \
         FROM DIGIT A, DIGIT B, DIGIT C, DIGIT E, DIGIT F) AS G WHERE N <= {ITEMS}"
    ))
    .unwrap();
    let dist = ["'DDDDDDDDDDDDDDDDDDDDDDDD'"; 10].join(", ");
    let old = "TIMESTAMP '2024-01-01 00:00:00'";
    let fresh = ORDERS - 9;
    for insert in [
        "INSERT INTO ITEM SELECT N, N, 'ITEM', MOD(N, 90) + 1.50, 'DATA' FROM N".to_owned(),
        format!("INSERT INTO STOCK SELECT 1, N, MOD(N, 90) + 10, {dist}, 0, 0, 0, 'DATA' FROM N"),
        format!(
            "INSERT INTO ORDERS SELECT 1, D_ID, N, MOD(N * 7, {CUSTOMERS}) + 1, CASE WHEN N > {fresh} THEN CAST(NULL AS SMALLINT) ELSE 3 END, 5, 1, {old} \
             FROM DISTRICT, N WHERE N <= {ORDERS}"
        ),
        format!(
            "INSERT INTO ORDER_LINE SELECT 1, D_ID, O.N, L.N, MOD(O.N * L.N, {ITEMS}) + 1, CAST(NULL AS TIMESTAMP(0)), 9.99, 1, 5, 'DDDDDDDDDDDDDDDDDDDDDDDD' \
             FROM DISTRICT, N AS O, N AS L WHERE O.N <= {ORDERS} AND L.N <= 5"
        ),
        format!("INSERT INTO NEW_ORDER SELECT 1, D_ID, N FROM DISTRICT, N WHERE N > {fresh} AND N <= {ORDERS}"),
    ] {
        s.run(&insert).unwrap();
    }
    s.run("DROP TABLE N RESTRICT").unwrap();
    s.run("DROP TABLE DIGIT RESTRICT").unwrap();
}

/// The tables and procedures of one warehouse defined in a database whose
/// statistics are gathered only when a test asks, in schema TPCC.
fn defined() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION TPCC").unwrap();
    let s = db.session("TPCC");
    for t in TABLES {
        s.run(t).unwrap();
        s.commit().unwrap();
    }
    for p in PROCEDURES.split("CREATE PROCEDURE").skip(1) {
        s.run(&format!("CREATE PROCEDURE{p}")).unwrap();
    }
    s.commit().unwrap();
    (db, s)
}

/// The warehouse's rows loaded by `s`.
fn filled(s: &Session) {
    let districts: Vec<String> =
        (1..=10).map(|d| format!("(1, {d}, 30000.00, 0.1000, {}, 'D', 'S1', 'S2', 'C', 'ST', '123456789')", ORDERS + 1)).collect();
    let mut customers = Vec::new();
    for d in 1..=10 {
        for c in 1..=CUSTOMERS {
            let credit = if c % 10 == 0 { "BC" } else { "GC" };
            customers.push(format!(
                "(1, {d}, {c}, 0.1000, '{credit}', '{}', 'F{c}', 50000.00, -10.00, 10.00, 1, 0, 'S1', 'S2', 'C', 'ST', '123456789', '1234567890123456', TIMESTAMP '2024-01-01 00:00:00', 'OE', 'DATA')",
                last_name(c % 10)
            ));
        }
    }
    s.run("INSERT INTO WAREHOUSE VALUES (1, 300000.00, 0.1000, 'W', 'S1', 'S2', 'C', 'ST', '123456789')").unwrap();
    load(s, "DISTRICT", &districts);
    load(s, "CUSTOMER", &customers);
    generated(s);
    s.commit().unwrap();
}

/// One warehouse loaded, its statistics gathered, its procedures defined,
/// in schema TPCC.
fn loaded() -> (Database, Session) {
    let (db, s) = defined();
    db.set_job_policy(JobPolicy::Automatic).unwrap();
    filled(&s);
    db.maintain().unwrap();
    (db, s)
}

/// An integer value.
fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The five transactions prepared, as the driver prepares them.
struct Calls {
    neword: Prepared,
    payment: Prepared,
    delivery: Prepared,
    ostat: Prepared,
    slev: Prepared,
}

impl Calls {
    fn of(s: &Session) -> Calls {
        let p = |sql: &str| s.prepare(sql).unwrap();
        Calls {
            neword: p("CALL NEWORD(?,?,?,?,?,?,?,?)"),
            payment: p("CALL PAYMENT(?,?,?,?,?,?,?,?)"),
            delivery: p("CALL DELIVERY(?,?)"),
            ostat: p("CALL OSTAT(?,?,?,?,?)"),
            slev: p("CALL SLEV(?,?,?)"),
        }
    }

    /// One NEWORD in each district, as the driver runs about ten for each
    /// DELIVERY, and one transaction of each other kind, by name and by id
    /// alike, each committed; the bindings and plannings each kind made.
    fn round(&self, s: &Session, r: &mut Sequence) -> [u64; 5] {
        let mut made = [0_u64; 5];
        for district in 1..=10 {
            made[0] = made[0].saturating_add(planned(s, &self.neword, &new_order(r, district)));
        }
        let byname = int(i64::from(r.between(1, 100) <= 60));
        let name = Value::Char(last_name(r.between(0, 9)));
        let (d, c) = (int(r.between(1, 10)), int(r.between(1, CUSTOMERS)));
        let payment = vec![int(1), d.clone(), int(1), d.clone(), c.clone(), byname.clone(), int(r.between(1, 5000)), name.clone()];
        let calls: [(&Prepared, Vec<Value>); 4] = [
            (&self.payment, payment),
            (&self.delivery, vec![int(1), int(r.between(1, 10))]),
            (&self.ostat, vec![int(1), d, c, byname, name]),
            (&self.slev, vec![int(1), int(r.between(1, 10)), int(r.between(10, 20))]),
        ];
        for (k, (p, values)) in calls.iter().enumerate() {
            made[k.saturating_add(1)] = planned(s, p, values);
        }
        made
    }
}

/// A NEWORD's values, in `district`: a customer, and five to fifteen items
/// of the one warehouse, all local, each of one unit.
fn new_order(r: &mut Sequence, district: i64) -> Vec<Value> {
    let count = r.between(5, 15);
    let items: Vec<Value> = (0..count).map(|_| int(r.between(1, ITEMS))).collect();
    let array = |v: Vec<Value>| Value::Array(Arc::from(v));
    vec![
        int(1),
        int(district),
        int(r.between(1, CUSTOMERS)),
        int(count),
        int(1),
        array(items),
        array(vec![int(1); usize::try_from(count).unwrap()]),
        array((0..count).map(|_| int(r.between(1, 10))).collect()),
    ]
}

/// The bindings and plannings one CALL of `p` with `values`, committed,
/// made (RULES.md D793).
fn planned(s: &Session, p: &Prepared, values: &[Value]) -> u64 {
    let before = bindings().wrapping_add(plannings());
    s.execute_prepared(p, values).unwrap();
    s.commit().unwrap();
    bindings().wrapping_add(plannings()).wrapping_sub(before)
}

/// Verifies: [DESIGN] RULES.md D613, D793, each TPC-C transaction, once it
/// has run a while, runs every later CALL with bindings and plans it kept:
/// no CALL binds or plans anything while NEW_ORDER, ORDERS, ORDER_LINE and
/// HISTORY grow, a SELECT ... INTO of a change's delta table neither its
/// query nor the change's object rows, those found by elements of the
/// order's arrays too, nor an INSERT.
#[test]
fn tpcc_procedures_plan_nothing_in_steady_state() {
    let (_db, s) = loaded();
    let calls = Calls::of(&s);
    let mut r = Sequence(7);
    for _ in 0..60 {
        calls.round(&s, &mut r);
    }
    let mut made = [0_u64; 5];
    for _ in 0..80 {
        let got = calls.round(&s, &mut r);
        for (total, n) in made.iter_mut().zip(got) {
            *total = total.saturating_add(n);
        }
    }
    assert_eq!(made, [0; 5], "bindings and plannings of NEWORD, PAYMENT, DELIVERY, OSTAT and SLEV in 80 rounds");
}

/// The plannings `n` executions of `p`, the query of one group's rows,
/// over groups 1 to `n`, made.
fn group_plannings(s: &Session, p: &Prepared, n: i64) -> u64 {
    let before = plannings();
    for g in 1..=n {
        let mut c = s.open_prepared(p, &[int(g)]).unwrap();
        while c.fetch().unwrap().is_some() {}
    }
    s.commit().unwrap();
    plannings().wrapping_sub(before)
}

/// `rows` rows into G: `per` in each group from `first`, keys from `from`.
fn grouped(s: &Session, (first, groups): (i64, i64), (per, from): (i64, i64)) {
    let rows: Vec<String> =
        (first..first + groups).flat_map(|g| (from..from + per).map(move |k| format!("({g}, {k}, {})", g * k % 7))).collect();
    for part in rows.chunks(200) {
        s.run(&format!("INSERT INTO G VALUES {}", part.join(", "))).unwrap();
    }
    s.commit().unwrap();
}

/// A database of the table G, its statistics gathered only when a test
/// asks (JOBS.POLICY MANUAL), and a session of its schema.
fn groups() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE G (G INTEGER NOT NULL, K INTEGER NOT NULL, V INTEGER, PRIMARY KEY (G, K))")
        .unwrap();
    let s = db.session("X");
    (db, s)
}

/// Verifies: [DESIGN] RULES.md D613, a plan kept that reads only through an
/// index's probes is kept as its table grows fivefold with its rows per
/// probe as they were, and is learned again once its rows per probe grow
/// past ten times what they were as it was learned. Its statistics are
/// gathered after each load, so each figure is measured from them, not
/// from whether a pass of the maintenance thread happened to come first
/// (RULES.md D473).
#[test]
fn a_probes_plan_is_learned_again_only_once_its_rows_per_probe_outgrow_it() {
    let (db, s) = groups();
    grouped(&s, (1, 100), (2, 1));
    db.analyze("X.G").unwrap();
    let p = s.prepare("SELECT K, V FROM G WHERE G = ?").unwrap();
    assert!(group_plannings(&s, &p, 40) > 0, "learned");
    assert_eq!(group_plannings(&s, &p, 40), 0, "kept");
    grouped(&s, (101, 400), (2, 1));
    db.analyze("X.G").unwrap();
    assert_eq!(group_plannings(&s, &p, 40), 0, "five times the rows, as many of each group");
    grouped(&s, (1, 500), (30, 3));
    db.analyze("X.G").unwrap();
    assert!(group_plannings(&s, &p, 40) > 0, "sixteen times the rows of each group");
}

/// Verifies: [DESIGN] RULES.md D613, a probes plan learned before its
/// table had statistics, its figure the whole table's rows, is learned
/// again once they are gathered, and so once its rows per probe then grow
/// past ten times what they were, as of one learned with them.
#[test]
fn a_probes_plan_learned_before_statistics_is_learned_again_once_they_are() {
    let (db, s) = groups();
    grouped(&s, (1, 100), (2, 1));
    let p = s.prepare("SELECT K, V FROM G WHERE G = ?").unwrap();
    assert!(group_plannings(&s, &p, 40) > 0, "learned without statistics");
    assert_eq!(group_plannings(&s, &p, 40), 0, "kept");
    db.analyze("X.G").unwrap();
    assert!(group_plannings(&s, &p, 40) > 0, "learned again with them");
    assert_eq!(group_plannings(&s, &p, 40), 0, "kept again");
    grouped(&s, (1, 500), (30, 3));
    db.analyze("X.G").unwrap();
    assert!(group_plannings(&s, &p, 40) > 0, "sixteen times the rows of each group");
}

/// The rows the CALLs of each procedure, NEWORD, PAYMENT, DELIVERY, OSTAT
/// and SLEV, have read in all, by their statement statistics.
fn rows_read(s: &Session) -> [u64; 5] {
    let read = |name: &str| {
        let sql = format!("SELECT ROWS_READ FROM INFORMATION_SCHEMA.STATEMENT_STATISTICS WHERE STATEMENT_TEXT LIKE 'CALL {name}%'");
        rows(s, &sql).unwrap().first().map_or(0, |n| n.trim().parse::<u64>().unwrap())
    };
    let found = ["NEWORD", "PAYMENT", "DELIVERY", "OSTAT", "SLEV"].map(read);
    s.commit().unwrap();
    found
}

/// The most rows a round's CALLs of each procedure read, by index: ten
/// NEWORDs of up to fifteen lines, their stock and keys; a PAYMENT by name;
/// a DELIVERY of ten districts' oldest orders and their lines; an ORDER
/// STATUS by name; a STOCK LEVEL of twenty orders' lines and their stock.
/// A scan of ORDER_LINE or NEW_ORDER, or of one district's lines, reads
/// more than any of them.
const ROUND_READS: [u64; 5] = [600, 60, 300, 60, 450];

/// Verifies: [DESIGN] RULES.md D839, D414, D45: the five TPC-C
/// procedures, defined and first CALLed while their tables were empty,
/// then run on the loaded warehouse with no statistics, then with
/// statistics gathered after a few rounds, which go stale as the orders
/// grow, read by index in every round: no round's CALLs of a procedure
/// read more rows than its index reads may.
///
/// Regression test (R-TEST-7): once the statistics were gathered, STOCK
/// LEVEL planned its order lines by a scan of ORDER_LINE's zones at each
/// run, as the key's range was estimated at several times its rows, and
/// the plan made with slots, a range of the key, never kept its shape.
#[test]
fn tpcc_procedures_read_by_index_under_any_statistics() {
    let (db, s) = defined();
    let calls = Calls::of(&s);
    s.execute_prepared(&calls.delivery, &[int(1), int(1)]).unwrap();
    s.execute_prepared(&calls.slev, &[int(1), int(1), int(15)]).unwrap();
    s.commit().unwrap();
    filled(&s);
    let mut r = Sequence(7);
    let mut before = rows_read(&s);
    for round in 0..30 {
        if round == 10 {
            for table in ["WAREHOUSE", "DISTRICT", "CUSTOMER", "NEW_ORDER", "ORDERS", "ORDER_LINE", "STOCK", "ITEM"] {
                db.analyze(&format!("TPCC.{table}")).unwrap();
            }
        }
        calls.round(&s, &mut r);
        let now = rows_read(&s);
        let each: Vec<u64> = now.iter().zip(before).map(|(n, b)| n.saturating_sub(b)).collect();
        assert!(each.iter().zip(ROUND_READS).all(|(n, most)| *n <= most), "round {round}: {each:?}");
        before = now;
    }
}
