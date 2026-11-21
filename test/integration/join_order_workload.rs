//! The join-order workload over an IMDB-shaped schema (RULES.md D308):
//! each of its 100 queries over a small generated database gives the same
//! rows with its joins enumerated as ordered greedily [X3.135-1986 §5.20
//! GR2b, §5.22].
//! The database and the queries are the joins benchmark's.

use turborel::db::Session;

use crate::common::{exec, sorted};

// The benchmark's database and queries, the plan-space test's too
// (`plan_space.rs`).
#[path = "../../bench/joins/imdb.rs"]
pub(super) mod imdb;

/// Titles of the small database.
const TITLES: u64 = 300;

/// The rows of `sql` with PLANNER.JOIN_ENUMERATION `word`.
fn planned(s: &Session, word: &str, sql: &str) -> Vec<String> {
    exec(s, &format!("SET PLANNER.JOIN_ENUMERATION = '{word}'")).unwrap();
    sorted(s, sql)
}

/// Verifies: [DESIGN] RULES.md D300-D304, D308: all 100 queries give the
/// greedy order's rows planned adaptively, and the data, skewed and
/// correlated, gives a fifth of them rows to find even at this size.
#[test]
fn every_query_agrees_with_the_greedy_order() {
    let db = imdb::load(TITLES);
    let s = db.session("IMDB");
    let queries = imdb::queries();
    assert_eq!(queries.len(), 100);
    let mut found = 0;
    for (name, sql) in queries {
        let adaptive = planned(&s, "ADAPTIVE", sql);
        assert_eq!(planned(&s, "GREEDY", sql), adaptive, "{name}");
        if adaptive.iter().any(|row| !row.split(',').all(|v| v == "NULL")) {
            found += 1;
        }
    }
    assert!(found >= 20, "{found} of 100 queries found rows");
}

/// The rows of `sql` with OPTIMIZER.SEMI_REDUCTION `word`.
fn reduced(s: &Session, word: &str, sql: &str) -> Vec<String> {
    exec(s, &format!("SET OPTIMIZER.SEMI_REDUCTION = '{word}'")).unwrap();
    sorted(s, sql)
}

/// Verifies: [DESIGN] RULES.md D542: every query of MIN alone gives the
/// same rows with the relations no MIN reads joined as semi joins as with
/// every join made, planned adaptively and greedily; and the first's plan
/// shows the semi joins.
#[test]
fn every_query_agrees_reduced_to_semi_joins() {
    let db = imdb::load(TITLES);
    let s = db.session("IMDB");
    let mut reduced_some = 0;
    for (name, sql) in imdb::queries() {
        for word in ["ADAPTIVE", "GREEDY"] {
            exec(&s, &format!("SET PLANNER.JOIN_ENUMERATION = '{word}'")).unwrap();
            let on = reduced(&s, "ON", sql);
            assert_eq!(reduced(&s, "OFF", sql), on, "{name} {word}");
        }
        exec(&s, "SET OPTIMIZER.SEMI_REDUCTION = 'ON'").unwrap();
        let plan = s.explain(sql, &turborel::params::Params::new()).unwrap();
        if plan.contains(turborel::optimizer::SEMI_REDUCTION) {
            reduced_some += 1;
            assert!(plan.contains("Semi"), "{name}: {plan}");
        }
    }
    assert!(reduced_some >= 50, "{reduced_some} of 100 queries reduced");
}

/// Verifies: [DESIGN] RULES.md D542: the rule leaves COUNT, SUM and a
/// relation a selected column reads, and makes semi joins of the
/// relations a MIN does not read.
#[test]
fn only_duplicate_blind_set_functions_are_reduced() {
    let db = imdb::load(TITLES);
    let s = db.session("IMDB");
    let applied = |sql: &str| {
        let plan = s.explain(sql, &turborel::params::Params::new()).unwrap();
        plan.lines().find(|l| l.starts_with("-- rules:")).unwrap_or_default().contains(turborel::optimizer::SEMI_REDUCTION)
    };
    let from = "FROM title AS t, movie_keyword AS mk, keyword AS k WHERE mk.movie_id = t.id AND mk.keyword_id = k.id";
    assert!(applied(&format!("SELECT MIN(t.title) {from}")));
    assert!(applied(&format!("SELECT COUNT(DISTINCT t.id), MAX(t.title) {from}")));
    assert!(!applied(&format!("SELECT COUNT(*) {from}")));
    assert!(!applied(&format!("SELECT SUM(t.production_year) {from}")));
    assert!(!applied(&format!("SELECT MIN(t.title), MIN(k.keyword), MIN(mk.id) {from}")));
    for sql in [format!("SELECT MIN(t.title) {from}"), format!("SELECT COUNT(DISTINCT k.keyword) {from}")] {
        assert_eq!(reduced(&s, "ON", &sql), reduced(&s, "OFF", &sql), "{sql}");
    }
}
