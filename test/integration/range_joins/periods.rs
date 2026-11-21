//! Period predicates of two tables' periods [ISO/IEC 9075-2:2023 §8.20]
//! as range joins and hash joins (RULES.md D581): of application-time
//! periods meeting, nested, equal and apart, and of a system-time period,
//! each predicate gives the rows the nested loop gives.

use turborel::db::Database;

use super::{agree, physical};
use crate::common::exec;

/// Schema Q: P of 200 application-time periods of one to nine days on a
/// grid of days, many of equal starts or ends, R of 60 likewise, and H
/// a system-versioned table of 30 rows, changed once.
fn periods() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION Q
           CREATE TABLE P (ID INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR VALIDITY (S, E))
           CREATE TABLE R (ID INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR VALIDITY (S, E))
           CREATE TABLE H (ID INTEGER, S TIMESTAMP(6) GENERATED ALWAYS AS ROW START,
             E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, PERIOD FOR SYSTEM_TIME (S, E)) WITH SYSTEM VERSIONING",
    )
    .unwrap();
    let s = db.session("Q");
    let period = |id: usize, start: usize, days: usize| {
        format!("({id}, DATE '2026-01-01' + INTERVAL '{start}' DAY(3), DATE '2026-01-01' + INTERVAL '{}' DAY(3))", start + days)
    };
    let p: Vec<String> = (0..200).map(|i| period(i, (i * 7) % 150, 1 + i % 9)).collect();
    let r: Vec<String> = (0..60).map(|i| period(i, (i * 11) % 150, 1 + (i * 5) % 9)).collect();
    s.run(&format!("INSERT INTO P VALUES {}", p.join(", "))).unwrap();
    s.run(&format!("INSERT INTO R VALUES {}", r.join(", "))).unwrap();
    let h: Vec<String> = (0..30).map(|i| format!("({i})")).collect();
    s.run(&format!("INSERT INTO H (ID) VALUES {}", h.join(", "))).unwrap();
    s.commit().unwrap();
    s.run("UPDATE H SET ID = ID + 100 WHERE ID < 10").unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR4b iv to ix, GR4a v]; [DESIGN]
/// RULES.md D580, D581: each predicate of two application-time periods,
/// and CONTAINS of a point in time, gives the nested loop's rows: of
/// order comparisons by a range join, of equalities by a hash join.
#[test]
fn each_period_predicate_joins_as_the_nested_loop_8_20() {
    let db = periods();
    let s = db.session("Q");
    for (predicate, ranged) in [
        ("P.VALIDITY OVERLAPS R.VALIDITY", true),
        ("P.VALIDITY CONTAINS R.VALIDITY", true),
        ("R.VALIDITY CONTAINS P.VALIDITY", true),
        ("P.VALIDITY PRECEDES R.VALIDITY", true),
        ("P.VALIDITY SUCCEEDS R.VALIDITY", true),
        ("P.VALIDITY CONTAINS R.S", true),
        ("P.VALIDITY EQUALS R.VALIDITY", false),
        ("P.VALIDITY IMMEDIATELY PRECEDES R.VALIDITY", false),
        ("P.VALIDITY IMMEDIATELY SUCCEEDS R.VALIDITY", false),
        ("P.VALIDITY OVERLAPS R.VALIDITY AND P.ID <> R.ID", true),
    ] {
        let sql = format!("SELECT P.ID, P.S, P.E, R.ID, R.S, R.E FROM P, R WHERE {predicate}");
        let joined = agree(&s, &sql, ranged);
        assert!(!joined.is_empty(), "{sql}");
        if !ranged {
            assert!(physical(&s, &sql).contains("HashJoin"), "{sql}");
        }
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR4]; [DESIGN] RULES.md D581: a
/// period predicate under NOT is judged whole, a null bound of an outer
/// join's missing row making it False, not Unknown; a written period,
/// whose bounds can raise, keeps the nested loop.
#[test]
fn predicates_not_a_conjunct_are_judged_whole_8_20() {
    let db = periods();
    let s = db.session("Q");
    let negated = "SELECT P.ID, R.ID FROM P, R WHERE NOT (P.VALIDITY OVERLAPS R.VALIDITY) AND P.ID < 3";
    assert_ne!(agree(&s, negated, false), Vec::<String>::new());
    let outer = "SELECT P.ID, R.ID FROM P LEFT JOIN R ON P.ID = R.ID + 1000 WHERE NOT (P.VALIDITY OVERLAPS R.VALIDITY)";
    assert_eq!(crate::common::rows(&s, outer).map(|r| r.len()), Ok(200));
    let written = "SELECT P.ID, R.ID FROM P, R WHERE PERIOD (P.S, P.E) OVERLAPS R.VALIDITY";
    assert_ne!(agree(&s, written, false), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20, §7.6 SR16]; [DESIGN] RULES.md
/// D580, D581: an application-time period overlapping a system-time
/// period of every row version read FOR SYSTEM_TIME FROM ... TO gives
/// the nested loop's rows.
#[test]
fn system_time_periods_join_as_the_nested_loop_8_20() {
    let db = periods();
    let s = db.session("Q");
    exec(&s, "UPDATE P SET S = DATE '1990-01-01', E = DATE '2999-01-01' WHERE ID < 5").unwrap();
    s.commit().unwrap();
    let sql = "SELECT P.ID, H.ID FROM P, H FOR SYSTEM_TIME FROM TIMESTAMP '1900-01-01 00:00:00' TO TIMESTAMP '2999-01-01 00:00:00' \
               WHERE P.VALIDITY OVERLAPS H.SYSTEM_TIME";
    let joined = agree(&s, sql, physical(&s, sql).contains("RangeJoin"));
    assert!(joined.len() >= 5 * 40, "{}", joined.len());
}
