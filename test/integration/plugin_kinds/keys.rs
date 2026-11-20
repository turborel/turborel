//! \[EXTENSION\] Indexes of many columns of an extension's index method
//! (RULES.md D226, D227), SORTED of the example: probes of a prefix of
//! the key, with the next column equal, compared or between bounds.

use super::kinds;
use crate::common::rows;
use turborel::params::Params;

/// T (K, V, W) of `n` rows, K the remainder by 10, V and W from 0, with an
/// index of SORTED on K and V.
fn keyed(s: &turborel::db::Session, n: u32) {
    s.run("CREATE TABLE T (K INTEGER, V INTEGER, W VARCHAR(10))").unwrap();
    let values: Vec<String> = (0..n).map(|v| format!("({}, {v}, 'w{v}')", v % 10)).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    s.run("CREATE INDEX T_KV ON T (K, V) USING SORTED").unwrap();
    s.commit().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D226: an index of two columns of a
/// method that SUPPORTS EQUALITY and RANGE serves a probe of its first
/// column equal, alone, with the second equal, compared, or between
/// bounds, each giving the rows a read of the table gives.
#[test]
fn an_index_of_two_columns_serves_prefixes() {
    let (_scratch, db) = kinds("plugin-keys-prefix", "");
    let s = db.session("U");
    keyed(&s, 100);
    let cases: [(&str, &[&str]); 5] = [
        ("K = 3 AND V = 13", &["13"]),
        ("K = 3 AND V > 80", &["83", "93"]),
        ("K = 3 AND V < 20", &["3", "13"]),
        ("K = 3 AND V BETWEEN 30 AND 60", &["33", "43", "53"]),
        ("K = 7 AND V >= 90", &["97"]),
    ];
    for (cond, want) in cases {
        let query = format!("SELECT V FROM T WHERE {cond} ORDER BY V");
        assert_eq!(rows(&s, &query).unwrap(), want, "{cond}");
        let plan = s.physical_plan(&query, &Params::new()).unwrap();
        assert!(plan.contains("T_KV"), "{cond}: {plan}");
    }
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE K = 9").unwrap(), ["10"]);
}
