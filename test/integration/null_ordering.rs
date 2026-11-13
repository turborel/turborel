//! `<null ordering>` [ISO/IEC 9075-2:2023 §10.10, Feature T611]: NULLS
//! FIRST and NULLS LAST after a sort key of a cursor's ORDER BY, of
//! ARRAY_AGG, of LISTAGG's WITHIN GROUP and of ARRAY(query); the ordering
//! implied, each in memory, spilled and by batches, an ordered read kept
//! only where it places nulls so, shown by EXPLAIN; and NULLS a key word
//! only there.

use turborel::db::{Completion, Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, SortOrder, Spelled, Value};

use crate::common::{exec, rows};

/// Schema A: T, of keys and names, nulls among both.
fn keyed() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N VARCHAR(5))").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 'a'), (NULL, 'b'), (3, NULL), (2, 'd'), (NULL, NULL)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The rows of `sql`, in order.
fn ordered(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.10 GR1c, GR1e ii, SR3]
/// NULLS FIRST puts the nulls before every value and NULLS LAST after,
/// ascending and descending alike, two nulls peers; without one, nulls
/// are last ascending and first descending (ID133); of a key by position,
/// of several keys each by its own.
#[test]
fn nulls_go_first_or_last_10_10() {
    let (_db, a) = keyed();
    let by = |order: &str| ordered(&a, &format!("SELECT K, N FROM T ORDER BY {order}"));
    let up_last = ["1,a", "2,d", "3,NULL", "NULL,b", "NULL,NULL"];
    let up_first = ["NULL,b", "NULL,NULL", "1,a", "2,d", "3,NULL"];
    let down_first = ["NULL,b", "NULL,NULL", "3,NULL", "2,d", "1,a"];
    let down_last = ["3,NULL", "2,d", "1,a", "NULL,b", "NULL,NULL"];
    assert_eq!(by("K, N"), up_last, "implied, ascending");
    assert_eq!(by("K ASC NULLS LAST, N"), up_last);
    assert_eq!(by("K NULLS FIRST, N"), up_first);
    assert_eq!(by("K DESC, N"), down_first, "implied, descending");
    assert_eq!(by("K DESC NULLS FIRST, N"), down_first);
    assert_eq!(by("K DESC NULLS LAST, N"), down_last);
    assert_eq!(by("1 NULLS FIRST, 2"), up_first, "a key by position");
    assert_eq!(by("N DESC NULLS LAST, K NULLS FIRST"), ["2,d", "NULL,b", "1,a", "NULL,NULL", "3,NULL"]);
    assert_eq!(by("N NULLS FIRST, K DESC NULLS LAST"), ["3,NULL", "NULL,NULL", "1,a", "NULL,b", "2,d"]);
}

/// Schema A: B, of 3,000 rows, and C, the same, a column store: K a key of
/// 97 values, a null every 11th row, S a string longer than a sort
/// prefix, of a null every 13th row, F a truth value, which has no sort
/// prefix, of a null every 7th row, I the row's number.
fn many() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE B (I INTEGER, K INTEGER, S VARCHAR(20), F BOOLEAN)
           CREATE TABLE C (I INTEGER, K INTEGER, S VARCHAR(20), F BOOLEAN) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["B", "C"] {
        for first in (0..3000).step_by(500) {
            let values: Vec<String> = (first..first + 500)
                .map(|i: i64| {
                    let k = if i % 11 == 0 { "NULL".to_owned() } else { (i * 31 % 97).to_string() };
                    let s = if i % 13 == 0 { "NULL".to_owned() } else { format!("'a shared start {}'", i % 17) };
                    let f = if i % 7 == 0 {
                        "NULL"
                    } else if i % 3 == 0 {
                        "TRUE"
                    } else {
                        "FALSE"
                    };
                    format!("({i}, {k}, {s}, {f})")
                })
                .collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// Whether `rows`, each "I,K", have the nulls of K where `order` places
/// them and its values in its direction: numbers, or truth
/// values, False before True.
fn placed(rows: &[String], order: SortOrder) -> bool {
    let SortOrder { descending, nulls_first } = order;
    let key = |k: &str| match k.to_ascii_uppercase().as_str() {
        "FALSE" => Some(0),
        "TRUE" => Some(1),
        other => other.parse::<i64>().ok(),
    };
    let keys: Vec<Option<i64>> = rows.iter().map(|r| r.split(',').nth(1).and_then(key)).collect();
    let nulls = keys.iter().filter(|k| k.is_none()).count();
    let (head, tail) = if nulls_first { keys.split_at(nulls) } else { keys.split_at(keys.len() - nulls) };
    let (values, gap) = if nulls_first { (tail, head) } else { (head, tail) };
    let in_order = values.windows(2).all(|w| if descending { w[0] >= w[1] } else { w[0] <= w[1] });
    nulls > 0 && gap.iter().all(Option::is_none) && values.iter().all(Option::is_some) && in_order
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.10 GR1e ii] [DESIGN]
/// RULES.md D31, D38: each direction under each null ordering, by an
/// integer, by a string longer than a sort prefix and by a truth value,
/// compared whole, sorts alike in
/// memory and spilled past MEMORY.WORK_BYTES, row by row and by batches,
/// of a table of rows and of a column store, and in the order it says.
#[test]
fn nulls_are_placed_in_memory_spilled_and_by_batches_10_10() {
    let (_db, a) = many();
    for (descending, nulls_first) in [(false, false), (false, true), (true, false), (true, true)] {
        let order = format!("{} NULLS {}", if descending { "DESC" } else { "ASC" }, if nulls_first { "FIRST" } else { "LAST" });
        for table in ["B", "C"] {
            let by_key = format!("SELECT I, K FROM {table} ORDER BY K {order}, I");
            let by_string = format!("SELECT I, S FROM {table} ORDER BY S {order}, I");
            let by_truth = format!("SELECT I, F FROM {table} ORDER BY F {order}, I");
            let all = || [&by_key, &by_string, &by_truth].map(|sql| ordered(&a, sql));
            let whole = all();
            assert!(placed(&whole[0], SortOrder { descending, nulls_first }), "{by_key}");
            assert!(placed(&whole[2], SortOrder { descending, nulls_first }), "{by_truth}");
            exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
            assert_eq!(all(), whole, "{order} {table}, spilled");
            exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
            assert_eq!(all(), whole, "{order} {table}, spilled, row by row");
            exec(&a, "SET MEMORY.WORK_BYTES = 4194304").unwrap();
            assert_eq!(all(), whole, "{order} {table}, row by row");
            exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
        }
    }
}

/// The physical plan EXPLAIN shows of `sql`.
fn physical(s: &Session, sql: &str) -> String {
    let explained = s.explain(sql, &Params::new()).unwrap();
    explained.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.10 GR1e ii] [DESIGN]
/// RULES.md D31: a column store kept in order of a column gives its rows
/// in that order, nulls greater than every value, so ORDER BY the column
/// in either direction under the null ordering implied reads it in order,
/// and under the other sorts; an index read in key order gives no null,
/// so it serves either null ordering. EXPLAIN shows a sort key's null
/// ordering where it is not the one implied.
#[test]
fn an_ordered_read_serves_the_null_ordering_it_places_10_10() {
    let (_db, a) = many();
    a.run("CREATE TABLE O (I INTEGER, K INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048, COLUMNSTORE.ORDER = 'K')").unwrap();
    a.run("INSERT INTO O SELECT I, K FROM B").unwrap();
    a.run("CREATE TABLE X (I INTEGER NOT NULL, K INTEGER)").unwrap();
    a.commit().unwrap();
    a.run("CREATE INDEX X_I ON X (I)").unwrap();
    a.run("INSERT INTO X SELECT I, K FROM B").unwrap();
    a.commit().unwrap();
    for (order, kept) in [("K", true), ("K DESC", true), ("K NULLS FIRST", false), ("K DESC NULLS LAST", false)] {
        let sql = format!("SELECT K FROM O ORDER BY {order}");
        let plan = physical(&a, &sql);
        assert_eq!(plan.contains("in order of") && !plan.contains("Sort"), kept, "{sql}: {plan}");
        assert_eq!(plan.contains("NULLS"), !kept, "{sql}: {plan}");
        let descending = order.contains("DESC");
        assert!(
            placed(
                &ordered(&a, &format!("SELECT I, K FROM O ORDER BY {order}")),
                SortOrder { descending, nulls_first: !kept ^ descending }
            ),
            "{sql}"
        );
    }
    for order in ["I", "I NULLS FIRST", "I DESC NULLS LAST"] {
        let sql = format!("SELECT I FROM X ORDER BY {order}");
        assert!(!physical(&a, &sql).contains("Sort"), "{sql}");
        let got = ordered(&a, &sql);
        assert_eq!((got.len(), got.first().map(String::as_str)), (3000, Some(if order.contains("DESC") { "2999" } else { "0" })), "{sql}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR11a, GR12c, §6.49
/// GR3, §10.10 GR1e ii] ARRAY_AGG, LISTAGG and ARRAY(query) order by keys
/// under NULLS FIRST and NULLS LAST; two ARRAY_AGG of one query differing
/// only in their null ordering are computed apart.
#[test]
fn ordered_aggregates_place_nulls_10_10() {
    let (_db, a) = keyed();
    let one = |sql: &str| ordered(&a, sql).join(";");
    assert_eq!(one("SELECT ARRAY_AGG(K ORDER BY K NULLS FIRST) FROM T"), "ARRAY[NULL, NULL, 1, 2, 3]");
    assert_eq!(one("SELECT ARRAY_AGG(K ORDER BY K DESC NULLS LAST) FROM T"), "ARRAY[3, 2, 1, NULL, NULL]");
    assert_eq!(
        one("SELECT ARRAY_AGG(N ORDER BY K NULLS FIRST, N), ARRAY_AGG(N ORDER BY K NULLS LAST, N) FROM T"),
        "ARRAY['b', NULL, 'a', 'd', NULL],ARRAY['a', 'd', NULL, 'b', NULL]",
        "computed apart"
    );
    assert_eq!(one("SELECT LISTAGG(N, ',') WITHIN GROUP (ORDER BY K NULLS FIRST, N) FROM T"), "b,a,d");
    assert_eq!(one("SELECT LISTAGG(N, ',') WITHIN GROUP (ORDER BY K DESC NULLS LAST, N DESC) FROM T"), "d,a,b");
    assert_eq!(one("SELECT ARRAY(SELECT K FROM T ORDER BY K NULLS FIRST) FROM T WHERE K = 1"), "ARRAY[NULL, NULL, 1, 2, 3]");
    assert_eq!(one("SELECT ARRAY(SELECT N FROM T ORDER BY N DESC NULLS LAST) FROM T WHERE K = 1"), "ARRAY['d', 'b', 'a', NULL, NULL]");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.10 GR1e ii; ISO/IEC
/// 9075:1992 §13.3] a SCROLL cursor FOR UPDATE, whose rows are found by
/// their sort keys, moves among them in the order its null ordering
/// gives.
#[test]
fn a_scroll_cursor_places_nulls_10_10() {
    let (_db, a) = keyed();
    let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
    let mut p = Params::new().declare("K_OUT", integer, Value::Null);
    a.declare_cursor("DECLARE C SCROLL CURSOR FOR SELECT K FROM T ORDER BY K NULLS FIRST FOR UPDATE OF N").unwrap();
    a.execute("OPEN C", &mut p).unwrap();
    let mut fetch = |how: &str| match a.execute(&format!("FETCH {how} FROM C INTO K_OUT"), &mut p) {
        Ok(Completion::NoData) => None,
        Ok(_) => Some(p.value("K_OUT").to_string()),
        Err(e) => panic!("{e:?}"),
    };
    assert_eq!(fetch("ABSOLUTE 3"), Some("1".to_owned()), "after the two nulls");
    assert_eq!(fetch("LAST"), Some("3".to_owned()));
    assert_eq!(fetch("ABSOLUTE 4"), Some("2".to_owned()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.10, §5.2] NULLS is a
/// key word only after a sort key, so a column may be named NULLS and
/// sorted under a null ordering; NULLS followed by neither FIRST nor LAST
/// is a syntax error (SQLCODE -104).
#[test]
fn nulls_names_a_column_10_10() {
    let (_db, a) = keyed();
    a.run("CREATE TABLE U (NULLS INTEGER)").unwrap();
    a.run("INSERT INTO U VALUES (2), (NULL), (1)").unwrap();
    assert_eq!(ordered(&a, "SELECT NULLS FROM U ORDER BY NULLS NULLS FIRST"), ["NULL", "1", "2"]);
    assert_eq!(ordered(&a, "SELECT NULLS FROM U ORDER BY NULLS DESC NULLS LAST"), ["2", "1", "NULL"]);
    for sql in
        ["SELECT K FROM T ORDER BY K NULLS", "SELECT K FROM T ORDER BY K NULLS MIDDLE", "SELECT ARRAY_AGG(K ORDER BY K NULLS) FROM T"]
    {
        assert_eq!(exec(&a, sql).err(), Some(Code::Syntax), "{sql}");
    }
}
