//! The Join Order Benchmark (RULES.md D308): each of its 113 queries over
//! a small generated database of its schema gives the same rows with its
//! joins enumerated as ordered greedily [X3.135-1986 §5.20 GR2b, §5.22].
//! The database and the queries are the joins benchmark's.

use turborel::db::Session;

use crate::common::{exec, sorted};

#[path = "../../bench/joins/imdb.rs"]
mod imdb;

/// Titles of the small database.
const TITLES: u64 = 300;

/// The rows of `sql` with PLANNER.JOIN_ENUMERATION `word`.
fn planned(s: &Session, word: &str, sql: &str) -> Vec<String> {
    exec(s, &format!("SET PLANNER.JOIN_ENUMERATION = '{word}'")).unwrap();
    sorted(s, sql)
}

/// Verifies: [DESIGN] RULES.md D300-D304, D308: all 113 queries give the
/// greedy order's rows planned adaptively, and the data, skewed and
/// correlated, gives a fifth of them rows to find even at this size.
#[test]
fn every_query_agrees_with_the_greedy_order() {
    let db = imdb::load(TITLES);
    let s = db.session("IMDB");
    let queries = imdb::queries();
    assert_eq!(queries.len(), 113);
    let mut found = 0;
    for (name, sql) in queries {
        let adaptive = planned(&s, "ADAPTIVE", sql);
        assert_eq!(planned(&s, "GREEDY", sql), adaptive, "{name}");
        if adaptive.iter().any(|row| !row.split(',').all(|v| v == "NULL")) {
            found += 1;
        }
    }
    assert!(found >= 20, "{found} of 113 queries found rows");
}
