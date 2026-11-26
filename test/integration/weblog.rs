//! The web log bench's rows and queries (RULES.md R-BENCH-1): the rows
//! `bench/analytics/weblog.rs` makes are the same from one seed whatever
//! part is made, come in the order the table keeps them in, and have the
//! shape measured of the reference log; and each of its 43 queries runs
//! on a table of them. The maker and the queries are the bench's own.

use std::collections::HashSet;
use std::sync::Arc;

use turborel::db::Database;
use turborel::params::Load;
use turborel::storage::lanes::Lanes;

#[path = "../../bench/analytics/weblog/columns.rs"]
mod columns;
#[path = "../../bench/analytics/weblog/keys.rs"]
mod keys;
#[path = "../../bench/analytics/weblog/layout.rs"]
mod layout;
#[path = "../../bench/analytics/weblog/measured.rs"]
mod measured;
#[path = "../../bench/analytics/weblog/plain.rs"]
mod plain;
#[path = "../../bench/analytics/weblog/queries.rs"]
mod queries;
#[path = "../../bench/analytics/weblog/random.rs"]
mod random;
#[path = "../../bench/analytics/weblog/rows.rs"]
mod rows;
#[path = "../../bench/analytics/weblog/schema.rs"]
mod schema;
#[path = "../../bench/analytics/weblog/texts.rs"]
mod texts;

use rows::Maker;

/// Rows of the small table.
const ROWS: u64 = 20_000;

/// The cells of the column `name` of `lanes`, as text.
fn column<'a>(lanes: &'a Lanes, name: &str) -> Vec<&'a str> {
    let at = schema::COLUMNS.iter().position(|(column, _)| *column == name).unwrap();
    (0..lanes.rows).map(|row| lanes.columns[at].cell(row).unwrap().unwrap()).collect()
}

/// How many of `cells` `wanted` holds of, in parts of a thousand.
fn share(cells: &[&str], wanted: impl Fn(&str) -> bool) -> usize {
    (cells.iter().filter(|cell| wanted(cell)).count() * 1000).div_euclid(cells.len())
}

/// Verifies: [TOOL] R-BENCH-1 the web log's rows depend on the seed and
/// the row's number alone: made twice they are the same, a part made
/// alone is that part of the whole, in every column, and another seed
/// makes other rows; a maker's label tells its rows and seed.
#[test]
fn the_same_rows_come_of_the_same_seed() {
    let maker = Maker::new(ROWS, 1);
    let whole = maker.lanes(0, 500);
    assert_eq!(whole, Maker::new(ROWS, 1).lanes(0, 500));
    assert_eq!(whole.columns.len(), schema::WIDTH);
    let part = maker.lanes(300, 100);
    for (name, _) in schema::COLUMNS {
        assert_eq!(column(&part, name), column(&whole, name)[300..400], "{name}");
    }
    assert_ne!(whole, Maker::new(ROWS, 2).lanes(0, 500));
    assert_eq!(maker.rows(), ROWS);
    assert!(maker.label().starts_with("20000 rows, seed 1"), "{}", maker.label());
}

/// Verifies: [TOOL] R-BENCH-1 the rows come in the order of the counter,
/// the day and the user, as the reference log's do, and have its measured
/// shape: the counter of most views has between a fifth and a quarter of
/// them, nine search phrases of ten are empty, nearly every view names
/// no advertising engine, a user has about five views, addresses begin
/// with a scheme, and every date is one of the measured days.
#[test]
fn the_rows_have_the_measured_shape() {
    let lanes = Maker::new(ROWS, 1).lanes(0, ROWS);
    let number = |name: &str| column(&lanes, name).iter().map(|cell| cell.parse::<i64>().unwrap()).collect::<Vec<_>>();
    let (counters, users, dates) = (number("CounterID"), number("UserID"), column(&lanes, "EventDate"));
    let keys: Vec<(i64, &str, i64)> = (0..lanes.rows).map(|row| (counters[row], dates[row], users[row])).collect();
    assert!(keys.windows(2).all(|pair| pair[0] <= pair[1]), "rows in the order of the counter, the day and the user");
    let busiest = measured::COUNTERS[0].to_string();
    let of_busiest = share(&column(&lanes, "CounterID"), |cell| cell == busiest);
    assert!((200..250).contains(&of_busiest), "{of_busiest}");
    let empty = share(&column(&lanes, "SearchPhrase"), str::is_empty);
    assert!((860..920).contains(&empty), "{empty}");
    assert!(share(&column(&lanes, "AdvEngineID"), |cell| cell == "0") > 970);
    let distinct = users.iter().collect::<HashSet<_>>().len();
    assert!((2_400..5_600).contains(&distinct), "{distinct} users of {ROWS} rows");
    assert!(share(&column(&lanes, "URL"), |cell| cell.starts_with("http")) > 990);
    let days: Vec<String> = measured::DAYS.as_chunks::<2>().0.iter().map(|[day, _]| format!("{}{day:02}", measured::MONTH)).collect();
    assert!(dates.iter().all(|date| days.iter().any(|day| day == date)));
    let times = column(&lanes, "EventTime");
    let before = |time: &str, date: &str| time.len() == 19 && time.get(..10).is_some_and(|day| day <= date);
    assert!(times.iter().zip(&dates).all(|(time, date)| before(time, date)), "a view's time is on or before its date");
}

/// Verifies: [TOOL] R-BENCH-1 each of the web log's 43 queries runs on a
/// table of its rows, loaded as the bench loads them, by column; the
/// count of all rows is the rows made, and the queries that ask for one
/// user, one referer and one page by number name ones the rows hold.
#[test]
fn every_query_runs_on_the_rows() {
    let maker = Maker::new(ROWS, 1);
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let s = db.session("B");
    for definition in schema::definitions() {
        s.run(&definition).unwrap();
    }
    let insert = s.prepare(&schema::insert()).unwrap();
    s.execute_prepared_lanes(&insert, Arc::new(maker.lanes(0, ROWS)), Load::Ending).unwrap();
    s.commit().unwrap();
    let constants = maker.constants();
    assert_eq!(queries::count(), 43);
    for n in 0..queries::count() {
        let (name, sql) = (queries::name(n), queries::text(n, &constants));
        assert!(!sql.contains('{'), "{name} has a constant left to replace: {sql}");
        let mut cursor = s.open_cursor(&sql).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut rows = Vec::new();
        while let Some(row) = cursor.fetch().unwrap_or_else(|e| panic!("{name}: {e}")) {
            rows.push(row);
        }
        drop(cursor);
        s.commit().unwrap();
        if name == "q01" {
            assert_eq!(rows[0][0].to_string(), ROWS.to_string());
        }
        if name == "q20" {
            assert!(!rows.is_empty(), "the user asked for by number has views");
        }
    }
    for (column, hash) in [("URLHash", constants.url), ("RefererHash", constants.referer)] {
        let mut cursor = s.open_cursor(&format!("SELECT COUNT(*) FROM hits WHERE {column} = {hash}")).unwrap();
        assert_ne!(cursor.fetch().unwrap().unwrap()[0].to_string(), "0", "{column} asked for by number occurs");
    }
}
