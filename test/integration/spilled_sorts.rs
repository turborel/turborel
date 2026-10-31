//! Spilled sorts (RULES.md D38): a sort past WORK_MEMORY writes its runs
//! with each row's prefix of its first key beside it and merges them by
//! those prefixes, giving the rows of every kind of key, ascending or
//! descending, nulls among them, in the order, stably, a sort held in
//! memory gives.

use turborel::db::{Database, Session};

use crate::common::{Counted, rows};
use crate::spilling::{SMALL, bytes};

/// Rows of `X`: many times what the smallest work area holds of them.
const ROWS: usize = 3000;

/// The columns of `X` sorted by, of each kind with prefixes: integers,
/// decimals, approximate numbers, strings longer and shorter than a
/// prefix, datetimes of each type, intervals of each class, and bit
/// strings longer and shorter than a prefix.
const COLUMNS: &[&str] = &["I", "D", "F", "C", "V", "DT", "TM", "TS", "TZ", "IY", "ID", "B"];

/// Minutes of `TZ`'s instants, of row `k`, from 06:00 UTC.
fn instant(k: usize) -> usize {
    (k * 97) % 900
}

/// Row `k` of `X`: a value null in some rows of most columns, and values
/// repeated, so sorts have ties.
fn x_row(k: usize) -> String {
    let null = |every: usize, value: String| if k.is_multiple_of(every) { "NULL".to_owned() } else { value };
    let cents = i64::try_from((k * 37) % 2000).unwrap() - 1000;
    let decimal = format!("{}{}.{:02}", if cents < 0 { "-" } else { "" }, cents.abs().div_euclid(100), cents.abs() % 100);
    let seconds = (k * 7) % 86_400;
    let time = format!("{:02}:{:02}:{:02}", seconds.div_euclid(3600), seconds.div_euclid(60) % 60, seconds % 60);
    let date = format!("{}-{:02}-{:02}", 2000 + k % 30, k % 12 + 1, (k * 5) % 28 + 1);
    // The instant written at a zone of -02:00 to +02:00, its local time
    // apart from it by as much.
    let zone = i64::try_from(k % 5).unwrap() * 60 - 120;
    let local = 360 + i64::try_from(instant(k)).unwrap() + zone;
    let tz = format!(
        "2026-10-28 {:02}:{:02}:00{}{:02}:{:02}",
        local.div_euclid(60),
        local % 60,
        if zone < 0 { '-' } else { '+' },
        zone.abs().div_euclid(60),
        zone.abs() % 60
    );
    let bits: String = (0..=(k * 7) % 80).map(|i| if (k.div_euclid(3) + i).is_multiple_of(3) { '1' } else { '0' }).collect();
    [
        k.to_string(),
        null(17, format!("{}", (k * 7919) % 500)),
        null(13, decimal),
        null(23, format!("{}E-1", (k * 31) % 1000)),
        null(11, format!("'a common prefix {:05}'", (k * 13) % 1500)),
        null(19, format!("'{}'", (k * 7) % 300)),
        null(7, format!("DATE '{date}'")),
        null(29, format!("TIME '{time}'")),
        null(31, format!("TIMESTAMP '{date} {time}'")),
        format!("TIMESTAMP '{tz}'"),
        null(37, format!("INTERVAL '{}-{}' YEAR TO MONTH", (k * 3) % 20, k % 12)),
        null(41, format!("INTERVAL '{} {:02}:{:02}:{:02}' DAY TO SECOND", k % 9, k % 24, (k * 7) % 60, k % 60)),
        null(43, format!("B'{bits}'")),
    ]
    .join(", ")
}

/// Schema S with table `X` of `ROWS` rows (`x_row`).
fn filled(db: &Database) -> Session {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S CREATE TABLE X (K INTEGER, I INTEGER, D DECIMAL(9, 2), F DOUBLE PRECISION,
           C CHARACTER(30), V CHARACTER VARYING(12), DT DATE, TM TIME, TS TIMESTAMP, TZ TIMESTAMP WITH TIME ZONE,
           IY INTERVAL YEAR TO MONTH, ID INTERVAL DAY TO SECOND, B BIT VARYING(80))",
    )
    .unwrap();
    let s = db.session("S");
    for first in (0..ROWS).step_by(250) {
        let values: Vec<String> = (first..first + 250).map(|k| format!("({})", x_row(k))).collect();
        s.run(&format!("INSERT INTO X VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    s
}

/// Sorts of `X` by `column`, each way: alone, ties in the order read; of
/// every column, then by `I` the other way.
fn sorts(column: &str) -> Vec<String> {
    ["ASC", "DESC"]
        .iter()
        .flat_map(|way| {
            let other = if *way == "ASC" { "DESC" } else { "ASC" };
            [format!("SELECT {column}, K FROM X ORDER BY {column} {way}"), format!("SELECT * FROM X ORDER BY {column} {way}, I {other}")]
        })
        .collect()
}

/// Verifies: [X3.135-1986 §8.3 GR3b]; [ISO/IEC 9075:1992 §8.2 GR4, GR5,
/// GR6]; [DESIGN] RULES.md D38: a sort past the smallest WORK_MEMORY,
/// merged by its rows' prefixes in passes, gives the rows of a key of every
/// kind with prefixes, ASC and DESC, nulls last ascending, then by another
/// key, in the order, stably, the sort held in memory gives, whatever
/// SPILL_BATCH.
#[test]
fn spilled_sorts_give_the_rows_held_sorts_give() {
    let db = Database::new();
    let s = filled(&db);
    let queries: Vec<String> = COLUMNS.iter().flat_map(|c| sorts(c)).collect();
    let held: Vec<Vec<String>> = queries.iter().map(|q| rows(&s, q).unwrap()).collect();
    assert!(held.iter().all(|r| r.len() == ROWS));
    assert_eq!(held[0].last().map(|r| r.starts_with("NULL")), Some(true), "nulls last, ascending");
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    for batch in [16_usize << 10, 4 << 10] {
        db.set_setting("SPILL_BATCH", &bytes(batch)).unwrap();
        for (q, expected) in queries.iter().zip(&held) {
            assert_eq!(&rows(&s, q).unwrap(), expected, "{q}, batch {batch}");
            assert!(counted.taken() > 0, "{q} spilled");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D38 [ISO/IEC 9075:1992 §8.2 GR5]:
/// timestamps WITH TIME ZONE sort by their instants in UTC, whatever zone
/// each is written at, held in memory and spilled alike, ties by the next
/// key.
#[test]
fn timestamps_with_time_zone_sort_by_their_instants() {
    let db = Database::new();
    let s = filled(&db);
    let mut expected: Vec<usize> = (0..ROWS).collect();
    expected.sort_by_key(|&k| (instant(k), k));
    let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
    let keys = |s: &Session| -> Vec<String> {
        rows(s, "SELECT K, TZ, C, D FROM X ORDER BY TZ, K").unwrap().iter().map(|r| r.split(',').next().unwrap().to_owned()).collect()
    };
    assert_eq!(keys(&s), expected, "held");
    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    assert_eq!(keys(&s), expected, "spilled");
}
