//! Joins larger than one probe per outer row suits: an adaptive join whose
//! windows outgrow a small work area and are partitioned and spilled, and
//! a join of two indexed tables that descends both trees together
//! (RULES.md D138). Each case's pairs are counted before timing and
//! checked against integer inclusion (R-BENCH-1).

use std::collections::HashMap;
use std::hint::black_box;

use super::harness::geometry::{ROWS, position, window};
use super::harness::{Bench, Case, note};
use turborel::db::Database;
use turborel::params::Params;
use turborel::value::{Decimal, Value};

const BROAD: u64 = 4_000;
const BROAD_WIDTH: u64 = 50;
const PARCELS: u64 = 100_000;
const PARCEL_WIDTH: u64 = 3;
const SPILLED: &str = "SELECT P.ID, W.ID FROM BROAD W JOIN POINTS P ON ST_INTERSECTS(P.G, W.G) = 1";
const PAIRED: &str = "SELECT P.ID, Q.ID FROM PARCELS Q JOIN POINTS P ON ST_INTERSECTS(P.G, Q.G) = 1";

pub fn measure(bench: &Bench, db: &Database) {
    fill(db, "BROAD", (BROAD, BROAD_WIDTH));
    let in_memory = counted(db, SPILLED, None);
    assert_eq!(in_memory, expected((BROAD, BROAD_WIDTH)), "in memory");
    assert_eq!(counted(db, SPILLED, Some("65536")), in_memory);
    for (work, label) in [(None, "work 4 MiB, scanned in memory"), (Some("65536"), "work 64 KiB, partitioned and spilled")] {
        let case = Case::new(format!("{ROWS} points, {BROAD} windows, width {BROAD_WIDTH}, {label}"), "joins");
        bench.measure(case, || {
            black_box(counted(db, SPILLED, work));
            1
        });
    }
    note(&format!("spilled join: {in_memory} pairs"));
    fill(db, "PARCELS", (PARCELS, PARCEL_WIDTH));
    db.create_index("PARCELS_G", "B.PARCELS", &["G"], "rtree").unwrap();
    db.analyze("B.POINTS").unwrap();
    db.analyze("B.PARCELS").unwrap();
    note(&format!("paired plan:\n{}", planned(db, PAIRED)));
    let paired = counted(db, PAIRED, None);
    assert_eq!(paired, expected((PARCELS, PARCEL_WIDTH)), "paired");
    bench.measure(Case::new(format!("{ROWS} points, {PARCELS} parcels, both indexed, as planned"), "joins"), || {
        black_box(counted(db, PAIRED, None));
        1
    });
    db.drop_index("PARCELS_G").unwrap();
    note(&format!("probed plan:\n{}", planned(db, PAIRED)));
    assert_eq!(counted(db, PAIRED, None), paired);
    bench.measure(Case::new(format!("{ROWS} points, {PARCELS} parcels, one tree probed per parcel"), "joins"), || {
        black_box(counted(db, PAIRED, None));
        1
    });
    note(&format!("paired join: {paired} pairs"));
}

fn fill(db: &Database, table: &str, (count, width): (u64, u64)) {
    let a = db.session("B");
    a.run(&format!("CREATE TABLE {table} (ID INTEGER, G GEOMETRY(0))")).unwrap();
    let insert = a.prepare(&format!("INSERT INTO {table} VALUES (?, ?)")).unwrap();
    for n in 0..count {
        a.execute_prepared(&insert, &[Value::Exact(Decimal::new(i128::from(n), 0)), window(n, width)]).unwrap();
    }
    a.commit().unwrap();
}

fn planned(db: &Database, query: &str) -> String {
    let a = db.session("B");
    let plan = a.physical_plan(query, &Params::new()).unwrap();
    a.commit().unwrap();
    plan
}

/// The pairs of `count` windows of `width` and the points, by integer
/// inclusion over the points at each position.
fn expected((count, width): (u64, u64)) -> u64 {
    let mut at: HashMap<(u64, u64), u64> = HashMap::new();
    for id in 0..ROWS {
        *at.entry(position(id)).or_default() += 1;
    }
    let mut pairs = 0;
    for n in 0..count {
        let (lx, ly) = position(n);
        for x in lx..=lx + width {
            for y in ly..=ly + width {
                pairs += at.get(&(x, y)).copied().unwrap_or(0);
            }
        }
    }
    pairs
}

/// The pairs `query` returns, under a work area of `work` bytes if given.
fn counted(db: &Database, query: &str, work: Option<&str>) -> u64 {
    let a = db.session("B");
    if let Some(work) = work {
        a.run(&format!("SET MEMORY.WORK_BYTES = {work}")).unwrap();
    }
    let mut cursor = a.open_cursor(query).unwrap();
    let mut count = 0;
    while let Some(row) = cursor.fetch().unwrap() {
        black_box(row);
        count += 1;
    }
    drop(cursor);
    a.commit().unwrap();
    count
}
