//! \[DESIGN\] The TPROC-C-style procedures the oltp bench installs, both
//! procedure sets, run compiled and by the interpreter (RULES.md D839):
//! each CALL's outputs, completion or error, and the tables as they end
//! are the same of both.

use turborel::db::{Database, Session};
use turborel::value::{Decimal, Value};

#[path = "../../bench/oltp/tpcc_procs_new.rs"]
mod new;
#[path = "../../bench/oltp/tpcc_procs.rs"]
mod old;
#[path = "../../bench/oltp/tpcc_schema.rs"]
mod schema;

/// An exact integer.
fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// An array of exact integers.
fn array(values: &[i64]) -> Value {
    Value::Array(values.iter().copied().map(int).collect::<Vec<_>>().into())
}

/// One warehouse of ten districts of five customers each, thirty items
/// and their stock, the procedures of `set` installed, compiled
/// procedures ON or OFF.
fn database(compiled: bool, set: &str) -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    a.run(&format!("SET EXECUTOR.COMPILED_PROCEDURES = '{}'", if compiled { "ON" } else { "OFF" })).unwrap();
    for t in schema::TABLES {
        a.run(t).unwrap();
    }
    a.commit().unwrap();
    for i in schema::INDEXES {
        a.run(i).unwrap();
    }
    a.run("INSERT INTO WAREHOUSE VALUES (1, 300000, 0.1000, 'W', 'S1', 'S2', 'C', 'ST', '123456789')").unwrap();
    for d in 1..=10 {
        a.run(&format!("INSERT INTO DISTRICT VALUES (1, {d}, 30000, 0.0500, 1, 'D{d}', 'S1', 'S2', 'C', 'ST', '123456789')")).unwrap();
        for c in 1..=5 {
            a.run(&format!(
                "INSERT INTO CUSTOMER VALUES (1, {d}, {c}, 0.0100, '{}', 'LAST{}', 'F{c}', 50000, -10, 10, 1, 0, \
                 'S1', 'S2', 'C', 'ST', '123456789', '0123456789012345', TIMESTAMP '2026-01-01 00:00:00', 'OE', 'data')",
                if c == 2 { "BC" } else { "GC" },
                c % 3
            ))
            .unwrap();
        }
    }
    for i in 1..=30 {
        a.run(&format!("INSERT INTO ITEM VALUES ({i}, {i}, 'item {i}', {}.50, 'data')", i % 7 + 1)).unwrap();
        let dists: Vec<String> = (1..=10).map(|d| format!("'dist {d} of {i}'")).collect();
        a.run(&format!("INSERT INTO STOCK VALUES (1, {i}, {}, {}, 0, 0, 0, 'data')", 10 + i % 15, dists.join(", "))).unwrap();
    }
    let procedures: &[&str] = if set == "old" {
        &[old::PAYMENT, old::OSTAT, old::NEWORD, old::DELIVERY, old::SLEV]
    } else {
        &[old::PAYMENT, old::OSTAT, new::NEWORD, new::DELIVERY, new::SLEV]
    };
    for p in procedures {
        a.run(p).unwrap();
    }
    a.commit().unwrap();
    (db, a)
}

/// What `call` with `values` gave in `s`: its completion and outputs, or
/// its error whole; committed if it succeeded, else rolled back.
fn called(s: &Session, call: &str, values: &[Value]) -> String {
    let gave = s.prepare(call).and_then(|p| s.execute_prepared_out(&p, values));
    let shown = match gave {
        Ok((done, out)) => format!("ok {done:?} {:?}", out.iter().map(ToString::to_string).collect::<Vec<_>>()),
        Err(e) => format!("error {} {} {e:?}", e.sqlstate().as_str(), e.sqlcode()),
    };
    if shown.starts_with("ok") {
        s.commit().unwrap();
    } else {
        s.rollback().unwrap();
    }
    format!("{call} {values:?}: {shown}")
}

/// The rows of `query`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// The tables as they are, their instants left out.
fn tables(s: &Session) -> Vec<String> {
    let mut all = Vec::new();
    for q in [
        "SELECT W_ID, W_YTD FROM WAREHOUSE",
        "SELECT D_ID, D_YTD, D_NEXT_O_ID FROM DISTRICT ORDER BY D_ID",
        "SELECT C_D_ID, C_ID, C_BALANCE, C_YTD_PAYMENT, C_PAYMENT_CNT, C_DELIVERY_CNT, C_DATA FROM CUSTOMER ORDER BY C_D_ID, C_ID",
        "SELECT H_C_ID, H_C_D_ID, H_D_ID, H_AMOUNT, H_DATA FROM HISTORY ORDER BY H_C_D_ID, H_C_ID, H_AMOUNT",
        "SELECT NO_D_ID, NO_O_ID FROM NEW_ORDER ORDER BY NO_D_ID, NO_O_ID",
        "SELECT O_D_ID, O_ID, O_C_ID, O_CARRIER_ID, O_OL_CNT, O_ALL_LOCAL FROM ORDERS ORDER BY O_D_ID, O_ID",
        "SELECT OL_D_ID, OL_O_ID, OL_NUMBER, OL_I_ID, OL_AMOUNT, OL_SUPPLY_W_ID, OL_QUANTITY, OL_DIST_INFO, \
         OL_DELIVERY_D IS NULL FROM ORDER_LINE ORDER BY OL_D_ID, OL_O_ID, OL_NUMBER",
        "SELECT S_I_ID, S_QUANTITY, S_YTD, S_ORDER_CNT, S_REMOTE_CNT FROM STOCK ORDER BY S_I_ID",
    ] {
        all.extend(rows(s, q));
    }
    all
}

/// The CALLs each set runs: orders entered, one with an item that is not
/// there, which rolls back; payments by number and by name; deliveries;
/// stock levels; order statuses.
fn script() -> Vec<(&'static str, Vec<Value>)> {
    let new_order = "CALL NEWORD(?,?,?,?,?,?,?,?)";
    let mut calls = Vec::new();
    for (d, c, items) in [(1, 1, vec![1, 2, 3]), (1, 2, vec![4, 5]), (2, 3, vec![6, 7, 8, 9]), (3, 1, vec![10, 99]), (1, 4, vec![11])] {
        let n = i64::try_from(items.len()).unwrap();
        let supply = vec![1; items.len()];
        let quantity: Vec<i64> = items.iter().map(|i| i % 9 + 1).collect();
        calls.push((new_order, vec![int(1), int(d), int(c), int(n), int(1), array(&items), array(&supply), array(&quantity)]));
    }
    let payment = "CALL PAYMENT(?,?,?,?,?,?,?,?)";
    for (d, c, by_name, last) in [(1, 1, 0, "LAST1"), (2, 0, 1, "LAST2"), (1, 2, 0, "LAST2"), (4, 0, 1, "LAST0")] {
        calls.push((payment, vec![int(1), int(d), int(1), int(d), int(c), int(by_name), int(125), Value::Char(last.to_owned())]));
    }
    calls.push(("CALL DELIVERY(?,?)", vec![int(1), int(7)]));
    calls.push(("CALL SLEV(?,?,?)", vec![int(1), int(1), int(15)]));
    calls.push(("CALL SLEV(?,?,?)", vec![int(1), int(2), int(30)]));
    calls.push(("CALL OSTAT(?,?,?,?,?)", vec![int(1), int(1), int(2), int(0), Value::Char("LAST2".to_owned())]));
    calls.push(("CALL OSTAT(?,?,?,?,?)", vec![int(1), int(2), int(0), int(1), Value::Char("LAST0".to_owned())]));
    calls.push(("CALL DELIVERY(?,?)", vec![int(1), int(3)]));
    calls
}

/// What the script gave of `set`, compiled or not, and the tables after.
fn ran(compiled: bool, set: &str) -> Vec<String> {
    let (_db, a) = database(compiled, set);
    let mut gave = Vec::new();
    // Twice, so the second round runs by the plans the first kept.
    for _ in 0..2 {
        for (call, values) in script() {
            gave.push(called(&a, call, &values));
        }
    }
    gave.extend(tables(&a));
    gave
}

/// Verifies: [DESIGN] RULES.md D839, the oltp bench's procedures, of both
/// sets, give the same compiled as interpreted.
#[test]
fn the_benchs_procedures_run_as_interpreted() {
    for set in ["old", "new"] {
        let interpreted = ran(false, set);
        let compiled = ran(true, set);
        for (n, (c, i)) in compiled.iter().zip(&interpreted).enumerate() {
            assert_eq!(c, i, "{set}, line {n}: compiled, then interpreted");
        }
        assert_eq!(compiled.len(), interpreted.len(), "{set}");
        let failed = compiled.iter().filter(|l| l.contains(": error")).count();
        assert!(failed <= 4, "{set}: only the order of an item not there fails, twice a round: {compiled:#?}");
    }
}
