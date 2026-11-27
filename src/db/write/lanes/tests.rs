use std::sync::Arc;

use super::BY_COLUMN;
use crate::db::Database;
use crate::params::Load;
use crate::storage::lanes::{Lane, Lanes, set_null};
use crate::threads::JobPolicy;

/// The lanes of `rows` rows of K and A, every seventh A null.
fn lanes(rows: usize) -> Lanes {
    let mut columns = vec![Lane { nulls: vec![0; rows.div_ceil(8)], ..Lane::default() }; 2];
    for n in 0..rows {
        for (c, lane) in columns.iter_mut().enumerate() {
            if c == 1 && n % 7 == 3 {
                set_null(&mut lane.nulls, n);
            } else {
                lane.bytes.extend_from_slice(n.to_string().as_bytes());
            }
            lane.ends.push(u32::try_from(lane.bytes.len()).unwrap());
        }
    }
    Lanes { rows, columns }
}

/// How many executions of a load of `rows` rows by column, into T as
/// `table` makes it, go in by column.
fn by_column(table: &str, rows: usize) -> usize {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION S {table}")).unwrap();
    let s = db.session("S");
    let p = s.prepare("INSERT INTO T (K, A) VALUES (?, ?)").unwrap();
    let before = BY_COLUMN.with(std::cell::Cell::get);
    s.execute_prepared_lanes(&p, Arc::new(lanes(rows)), Load::Going).unwrap();
    s.execute_prepared_lanes(&p, Arc::new(Lanes::default()), Load::Ending).unwrap();
    s.commit().unwrap();
    BY_COLUMN.with(std::cell::Cell::get).saturating_sub(before)
}

/// Verifies: [DESIGN] RULES.md D1148: a load by column into a table of the
/// column store that qualifies goes in by column, a CHECK and a default
/// with it, and, by RULES.md D1235, a primary key; one into a table of
/// the row store, or set to load through its tree, goes in as rows; so do
/// more rows than a group takes.
#[test]
fn a_load_goes_by_column_only_where_nothing_needs_a_row() {
    let store = "USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 4096";
    assert_eq!(by_column(&format!("CREATE TABLE T (K INTEGER NOT NULL, A INTEGER CHECK (A >= 0), B INTEGER DEFAULT 3) {store})"), 4000), 1);
    assert_eq!(by_column(&format!("CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER) {store})"), 4000), 1);
    assert_eq!(by_column(&format!("CREATE TABLE T (K INTEGER, A INTEGER) {store}, COLUMNSTORE.LOAD = 'TREE')"), 4000), 0);
    assert_eq!(by_column("CREATE TABLE T (K INTEGER, A INTEGER)", 4000), 0);
    assert_eq!(by_column(&format!("CREATE TABLE T (K INTEGER, A INTEGER) {store})"), 5000), 0);
}
