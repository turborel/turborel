//! Index lookups for WHERE function calls: reject extra candidates, preserve
//! errors and use each execution's parameter (RULES.md D15, D34, D45).

mod fixture;

use fixture::{Function, Method, database, indexed, plan, session};
use std::sync::atomic::Ordering;
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{fetched, number, rows};

/// Verifies: [DESIGN] D34: NEXT_TO(K, 40) returns 41 even when the index
/// also returns 42. Exact and lossy lookups both check the full WHERE
/// condition and give the same rows as a scan, including nulls and empty
/// results. Reversing the arguments keeps their meaning. Removing the
/// WHERE check lets an extra row through and fails this test (R-TEST-9).
/// Rows are fetched even when the index covers the projection; filtering
/// preserves the index's forward or reverse order.
#[test]
fn candidates_are_rechecked_against_the_whole_condition() {
    for lossy in [true, false] {
        let method = Method { lossy, ..Method::default() };
        let seen = method.seen.clone();
        let db = database(method, Function::default());
        let s = session(&db);
        let queries = [
            "SELECT N FROM T WHERE NEXT_TO(K, 40)",
            "SELECT N FROM T WHERE NEXT_TO(40, K)",
            "SELECT N FROM T WHERE NEXT_TO(K, 40) AND V = 42",
            "SELECT N FROM T WHERE NEXT_TO(K, CAST(NULL AS INTEGER))",
            "SELECT N FROM T WHERE NEXT_TO(K, 900)",
            "SELECT K FROM T WHERE NEXT_TO(K, 40) ORDER BY K",
            "SELECT K FROM T WHERE NEXT_TO(K, 40) ORDER BY K DESC",
        ];
        let scanned: Vec<_> = queries.iter().map(|sql| rows(&s, sql).unwrap()).collect();
        s.commit().unwrap();
        indexed(&db, &["K"]);
        for (sql, expected) in queries.iter().zip(scanned) {
            assert_eq!(rows(&s, sql).unwrap(), expected, "{sql}");
            let shown = plan(&s, sql);
            assert!(shown.contains("IndexScan") && shown.contains("Filter NEXT_TO"), "{shown}");
            assert!(!shown.contains("IndexOnlyScan"), "{shown}");
        }
        assert!(seen.lock().unwrap().iter().any(|(_, argument, _)| *argument == 1));
    }
}

/// Verifies: [DESIGN] D15: a function that raises SQLCODE -301 still does
/// so with an index present, even on a row a lookup would skip or reject.
/// A division in an argument or another WHERE term also keeps the full
/// scan and raises SQLCODE -301.
#[test]
fn raising_predicates_and_conjuncts_keep_their_errors() {
    for error_at in [0, 41, 42] {
        let function = Function { safe: false, error_at: Some(error_at), ..Function::default() };
        let db = database(Method { lossy: true, ..Method::default() }, function);
        let s = session(&db);
        let sql = "SELECT N FROM T WHERE NEXT_TO(K, 40) AND N = 100";
        assert_eq!(rows(&s, sql), Err(Code::Data));
        s.rollback().unwrap();
        indexed(&db, &["K"]);
        assert!(plan(&s, sql).contains("SeqScan"));
        assert_eq!(fetched(&s, sql).unwrap_err().sqlcode(), -301);
    }
    let db = database(Method::default(), Function::default());
    indexed(&db, &["K"]);
    let s = session(&db);
    for sql in ["SELECT N FROM T WHERE NEXT_TO(K, 40) AND 1 / N = 1", "SELECT N FROM T WHERE NEXT_TO(K, 1 / N)"] {
        assert!(plan(&s, sql).contains("SeqScan"));
        assert_eq!(fetched(&s, sql).unwrap_err().sqlcode(), -301);
        s.rollback().unwrap();
    }
}

/// Verifies: [DESIGN] D34, D45: reusing a prepared plan updates both the
/// index lookup and the WHERE check with the current parameter, including
/// null and a string converted to an integer. A bad conversion raises
/// SQLCODE -301 before and after the plan is learned for reuse.
#[test]
fn learned_predicates_fill_each_executions_argument() {
    let method = Method { lossy: true, ..Method::default() };
    let seen = method.seen.clone();
    let priced = method.priced.clone();
    let db = database(method, Function::default());
    indexed(&db, &["K"]);
    let s = session(&db);
    for sql in ["SELECT N FROM T WHERE NEXT_TO(K, ?)", "SELECT N FROM T WHERE NEXT_TO(?, K)"] {
        let prepared = s.prepare(sql).unwrap();
        assert_eq!(s.open_prepared(&prepared, &[Value::Char("bad".into())]).err().unwrap().sqlcode(), -301);
        for n in 20..28 {
            let mut cursor = s.open_prepared(&prepared, &[number(n)]).unwrap();
            let expected = if sql.contains("K, ?") { n + 1 } else { n - 1 };
            assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), expected.to_string());
            assert!(cursor.fetch().unwrap().is_none());
        }
        let before = seen.lock().unwrap().len();
        let estimates = priced.load(Ordering::Relaxed);
        let mut cursor = s.open_prepared(&prepared, &[Value::Null]).unwrap();
        assert!(cursor.fetch().unwrap().is_none());
        assert_eq!(seen.lock().unwrap().len(), before + 1, "the learned index still runs");
        assert!(seen.lock().unwrap().last().unwrap().2.is_null());
        assert_eq!(priced.load(Ordering::Relaxed), estimates, "a learned plan is not priced again");
        assert_eq!(s.open_prepared(&prepared, &[Value::Char("bad".into())]).err().unwrap().sqlcode(), -301);
        let mut cursor = s.open_prepared(&prepared, &[Value::Char("30".into())]).unwrap();
        assert!(cursor.fetch().unwrap().is_some());
    }
}

/// Verifies: [DESIGN] D15, D34: MC/DC pairs independently remove the
/// function's opt-in, determinism or non-raising arguments; each prevents
/// the lookup. A function that returns true for nulls without opting in
/// keeps those rows. An unsupported argument position or column keeps a
/// scan. Wrong argument types or counts raise SQLCODE -103.
#[test]
fn only_declared_safe_supported_calls_are_probed() {
    for (safe, deterministic, argument, uses_index) in
        [(true, true, "40", true), (false, true, "40", false), (true, false, "40", false), (true, true, "N / N", false)]
    {
        let db = database(Method::default(), Function { safe, deterministic, ..Function::default() });
        indexed(&db, &["K"]);
        let shown = plan(&session(&db), &format!("SELECT N FROM T WHERE NEXT_TO(K, {argument})"));
        assert_eq!(shown.contains("IndexScan"), uses_index, "{shown}");
    }
    let db = database(Method::default(), Function { safe: false, null_true: true, ..Function::default() });
    indexed(&db, &["K"]);
    let s = session(&db);
    assert_eq!(rows(&s, "SELECT N FROM T WHERE NEXT_TO(K, 40) ORDER BY N").unwrap(), ["41", "200"]);
    let db = database(Method { argument: Some(0), ..Method::default() }, Function::default());
    indexed(&db, &["K"]);
    let s = session(&db);
    for sql in ["SELECT N FROM T WHERE NEXT_TO(40, K)", "SELECT N FROM T WHERE NEXT_TO(N, 40)", "SELECT N FROM T WHERE NEXT_TO(K, N)"] {
        assert!(plan(&s, sql).contains("SeqScan"));
    }
    assert!(plan(&s, "SELECT N FROM T WHERE NEXT_TO(K, 40)").contains("IndexScan"));
    assert_eq!(s.open_cursor("SELECT N FROM T WHERE NEXT_TO(K, 'bad')").err().unwrap().sqlcode(), -103);
    assert_eq!(s.open_cursor("SELECT N FROM T WHERE NEXT_TO(K)").err().unwrap().sqlcode(), -103);
}

/// Verifies: [DESIGN] D34: the function can use any column of a composite
/// key. MC/DC pairs vary whether a nullable column is tested by the call
/// and whether an untested column is NOT NULL. Only an untested nullable
/// column prevents the lookup, since omitted null keys could hide matches.
/// An empty table returns no rows, even for a function that could raise.
#[test]
fn composite_keys_and_empty_tables_keep_all_matches() {
    for (columns, uses_index) in [(&["K"][..], true), (&["K", "N"][..], true), (&["N", "K"][..], true), (&["K", "V"][..], false)] {
        let db = database(Method::default(), Function::default());
        indexed(&db, columns);
        let s = session(&db);
        let sql = "SELECT N FROM T WHERE NEXT_TO(K, 40)";
        assert_eq!(plan(&s, sql).contains("IndexScan"), uses_index);
        assert_eq!(rows(&s, sql).unwrap(), ["41"]);
        s.run("DELETE FROM T").unwrap();
        assert_eq!(rows(&s, sql).unwrap(), Vec::<String>::new());
    }
    let db = database(Method::default(), Function { safe: false, error_at: Some(0), ..Function::default() });
    indexed(&db, &["K"]);
    let s = session(&db);
    s.run("DELETE FROM T").unwrap();
    assert_eq!(rows(&s, "SELECT N FROM T WHERE NEXT_TO(K, 40)").unwrap(), Vec::<String>::new());
}

/// Verifies: [DESIGN] D34, D29: an empty function lookup still records a
/// read for transaction conflict checks. Two serializable readers each
/// insert a row that would match the other's lookup; they cannot both
/// commit, and the second commit raises SQLCODE -911.
#[test]
fn predicate_reads_record_concurrent_matches() {
    let db = database(Method::default(), Function::default());
    indexed(&db, &["K"]);
    crate::common::serializable(&db);
    let (a, b) = (session(&db), session(&db));
    for (s, n) in [(&a, 500), (&b, 600)] {
        let sql = format!("SELECT N FROM T WHERE NEXT_TO(K, {n})");
        assert!(plan(s, &sql).contains("IndexScan"));
        assert_eq!(rows(s, &sql).unwrap(), Vec::<String>::new());
    }
    a.run("INSERT INTO T VALUES (601, 601, NULL)").unwrap();
    b.run("INSERT INTO T VALUES (501, 501, NULL)").unwrap();
    a.commit().unwrap();
    assert_eq!(b.commit().unwrap_err().sqlcode(), -911);
}

/// Verifies: [DESIGN] D34: old index entries cannot make a changed row
/// match its former key. UPDATE and DELETE, including prepared writes,
/// check the full WHERE condition so extra candidates are left unchanged.
/// Final rows equal those produced by a full scan.
#[test]
fn writes_and_visible_versions_recheck_candidates() {
    let mut results = Vec::new();
    for with_index in [false, true] {
        let db = database(Method { lossy: true, ..Method::default() }, Function::default());
        if with_index {
            indexed(&db, &["K"]);
        }
        let s = session(&db);
        let update = s.prepare("UPDATE T SET V = 900 WHERE NEXT_TO(K, ?)").unwrap();
        s.execute_prepared(&update, &[number(40)]).unwrap();
        s.execute_prepared(&update, &[number(50)]).unwrap();
        s.run("DELETE FROM T WHERE NEXT_TO(K, 60)").unwrap();
        s.run("UPDATE T SET K = 400 WHERE N = 41").unwrap();
        assert_eq!(rows(&s, "SELECT N FROM T WHERE NEXT_TO(K, 40)").unwrap(), Vec::<String>::new());
        results.push(rows(&s, "SELECT N, V FROM T WHERE N IN (41, 42, 51, 52, 61, 62) ORDER BY N").unwrap());
    }
    assert_eq!(results[0], results[1]);
    assert_eq!(results[1], ["41,900", "42,42", "51,900", "52,52", "62,62"]);
}

/// Verifies: [DESIGN] D34: an unknown candidate count is priced as fetching
/// every table row. An expensive page estimate makes the planner choose a
/// scan; an unknown page estimate uses the usual tree descent cost. Each
/// choice returns the same rows.
#[test]
fn estimates_price_candidates_and_unknown_costs() {
    for (estimate, pages, uses_index) in
        [(Some(2), Some(1.0), true), (None, Some(1.0), false), (Some(2), None, true), (Some(2), Some(1.0e9), false)]
    {
        let db = database(Method { estimate, pages, ..Method::default() }, Function::default());
        indexed(&db, &["K"]);
        let s = session(&db);
        let sql = "SELECT N FROM T WHERE NEXT_TO(K, 40)";
        let shown = plan(&s, sql);
        assert_eq!(shown.contains("IndexScan"), uses_index, "{shown}");
        assert_eq!(rows(&s, sql).unwrap(), ["41"]);
    }
}

/// Verifies: [DESIGN] D34: built-in indexes decline function lookups,
/// return no estimates and reject a direct search with SQLCODE -900.
/// Storage treats every key and range as a possible match, including
/// nulls, without calling a function that would raise.
#[test]
fn builtins_refuse_predicates_and_storage_never_evaluates_them() {
    use std::sync::Arc;
    use turborel::functions::ScalarRef;
    use turborel::index::{BTreeMethod, HashMethod, Index, IndexMethod, KeyOrder, Predicate, Probe};
    use turborel::storage::Condition;

    let function = Function { safe: false, error_at: Some(0), ..Function::default() };
    let predicate = Box::new(Predicate { function: ScalarRef(Arc::new(function)), column: 0, argument: 0 });
    let probe = Probe::Predicate { predicate: predicate.clone(), value: number(0) };
    let methods: Vec<Box<dyn IndexMethod>> = vec![Box::new(BTreeMethod::in_memory()), Box::new(HashMethod::in_memory())];
    for method in methods {
        assert!(!method.supports_predicate(&predicate, &[fixture::INTEGER]));
        let index: Arc<dyn Index> = Arc::from(method.create(&[fixture::INTEGER]).unwrap());
        assert!(index.estimate(&probe).is_none());
        assert!(index.probe_pages(&probe).is_none());
        assert_eq!(index.search(&probe, KeyOrder::Ascending).next().unwrap().unwrap_err().sqlcode(), -900);
    }
    let condition = Condition { columns: vec![0], probe };
    for value in [number(0), Value::Null] {
        assert!(condition.probe.answers([&value]));
        assert!(condition.matches(&vec![value]));
    }
    assert!(condition.may_match(&[None]));
    assert!(condition.may_match(&[]));
    assert!(condition.may_match(&[Some((number(0), number(0)))]));
}

/// Verifies: [DESIGN] D34, D66: filtering K can leave nulls in V when the
/// index holds null keys. SQL's requested null order still needs a sort,
/// in either direction and with FETCH FIRST. Removing the planner's check
/// for surviving nulls makes this regression test fail (R-TEST-7).
#[test]
fn nullable_candidates_keep_the_requested_null_order() {
    let db = database(Method { lossy: true, ..Method::default() }, Function::default());
    db.set_setting("TABLES.UNIQUE_INDEX_METHOD", &Value::Char(fixture::METHOD.into())).unwrap();
    let s = session(&db);
    s.run("INSERT INTO T VALUES (41, 201, NULL), (41, 202, 1)").unwrap();
    s.commit().unwrap();
    let orders = [
        "K, V",
        "K, V NULLS FIRST",
        "K, V NULLS LAST",
        "K DESC, V DESC",
        "K DESC, V DESC NULLS FIRST",
        "K DESC, V DESC NULLS LAST",
        "K, V FETCH FIRST 1 ROW ONLY",
        "K DESC, V DESC FETCH FIRST 1 ROW ONLY",
    ];
    let queries: Vec<_> = orders.iter().map(|order| format!("SELECT K, V FROM T WHERE NEXT_TO(K, 40) ORDER BY {order}")).collect();
    let scanned: Vec<_> = queries.iter().map(|sql| rows(&s, sql).unwrap()).collect();
    s.commit().unwrap();
    s.run("ALTER TABLE T ADD CONSTRAINT T_NULL_KEY UNIQUE NULLS NOT DISTINCT (K, V)").unwrap();
    s.commit().unwrap();
    for (sql, expected) in queries.iter().zip(scanned) {
        assert_eq!(rows(&s, sql).unwrap(), expected, "{sql}");
        let shown = plan(&s, sql);
        assert!(shown.contains("IndexScan") && shown.contains("Sort"), "{shown}");
    }
}
