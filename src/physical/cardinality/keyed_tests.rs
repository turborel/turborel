//! Unit tests of the join estimates from keys and bounds (RULES.md D384,
//! D385): a join on a composite key, on a referential constraint, the
//! bound of an equality to a key, bounds never below the true rows, and
//! the pairs' corrections giving one answer for every order of a set.

use super::Estimates;
use super::joins::Classes;
use crate::db::{Database, Session};

/// Schema H, shaped as an order's lines and a part's offers: PS of 800
/// rows, each of 200 parts offered by 4 of 20 suppliers, its key both,
/// QTY each offer's number; LI of 2,000 lines, each of one offer, with
/// no constraint; LF the same lines, referencing PS; P of 200 parts, its
/// key PK. Statistics gathered.
fn database() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION H
           CREATE TABLE PS (PK INTEGER NOT NULL, SK INTEGER NOT NULL, QTY INTEGER NOT NULL, PRIMARY KEY (PK, SK))
           CREATE TABLE LI (OK INTEGER NOT NULL, PK INTEGER NOT NULL, SK INTEGER NOT NULL)
           CREATE TABLE LF (OK INTEGER NOT NULL, PK INTEGER NOT NULL, SK INTEGER NOT NULL,
             FOREIGN KEY (PK, SK) REFERENCES PS (PK, SK))
           CREATE TABLE P (PK INTEGER NOT NULL PRIMARY KEY, SZ INTEGER NOT NULL)",
    )
    .unwrap();
    let s = db.session("H");
    let rows = |n: i32, f: &dyn Fn(i32) -> String| (0_i32..n).map(f).collect::<Vec<_>>().join(", ");
    let offer = |i: i32| {
        let (part, nth) = (i.rem_euclid(200_i32), i.div_euclid(200_i32).rem_euclid(4_i32));
        (part, part.wrapping_add(nth.wrapping_mul(5_i32)).rem_euclid(20_i32))
    };
    s.run(&format!("INSERT INTO PS VALUES {}", rows(800_i32, &|i| format!("({}, {}, {i})", offer(i).0, offer(i).1)))).unwrap();
    for table in ["LI", "LF"] {
        s.run(&format!("INSERT INTO {table} VALUES {}", rows(2_000_i32, &|i| format!("({i}, {}, {})", offer(i).0, offer(i).1)))).unwrap();
    }
    s.run(&format!("INSERT INTO P VALUES {}", rows(200_i32, &|k| format!("({k}, {})", k % 50_i32)))).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    (db, s)
}

/// The estimated rows and bound of `sql`'s optimized plan.
fn card(s: &Session, sql: &str) -> (f64, f64) {
    let (st, plan) = s.optimized(sql).unwrap();
    let card = Estimates::new(&st).card(&plan);
    (card.rows, card.upper)
}

/// The rows `sql`, a query of one COUNT(*), counts.
fn count(s: &Session, sql: &str) -> f64 {
    let text = s.open_cursor(sql).unwrap().fetch().unwrap().unwrap()[0].to_string();
    s.commit().unwrap();
    text.parse::<f64>().unwrap()
}

/// Whether `estimate` is within one and a half times `actual`.
fn close(estimate: f64, actual: f64) -> bool {
    estimate <= 1.5_f64 * actual && estimate >= actual / 1.5_f64
}

/// Verifies: [DESIGN] RULES.md D384: a join of every line to its one
/// offer, on both columns of the offers' key, is estimated as the lines'
/// rows, not as one in each column's distinct values multiplied, which
/// gives a fifth of them; and so with the lines filtered.
#[test]
fn a_join_on_a_composite_key_keeps_each_line_once() {
    let (_db, s) = database();
    let (whole, _) = card(&s, "SELECT * FROM LI, PS WHERE LI.PK = PS.PK AND LI.SK = PS.SK");
    assert!(close(whole, 2_000.0_f64), "the lines' rows: {whole}");
    let (filtered, _) = card(&s, "SELECT * FROM LI, PS WHERE LI.PK = PS.PK AND LI.SK = PS.SK AND LI.OK < 1000");
    assert!(close(filtered, 1_000.0_f64), "the filtered lines' rows: {filtered}");
    let (alone, _) = card(&s, "SELECT * FROM LI, PS WHERE LI.PK = PS.PK");
    assert!(close(alone, 8_000.0_f64), "one column alone is not corrected: {alone}");
}

/// Verifies: [DESIGN] RULES.md D384: a join on a referential constraint
/// whose referencing columns are NOT NULL keeps the referencing rows
/// times the share of the referenced rows the other filter keeps.
#[test]
fn a_join_on_a_referential_constraint_is_exact() {
    let (_db, s) = database();
    let sql = "SELECT * FROM LF, PS WHERE LF.PK = PS.PK AND LF.SK = PS.SK AND PS.QTY < 400";
    let (rows, _) = card(&s, sql);
    let truth = count(&s, "SELECT COUNT(*) FROM LF, PS WHERE LF.PK = PS.PK AND LF.SK = PS.SK AND PS.QTY < 400");
    assert!(close(rows, truth), "{rows} of {truth}");
}

/// Verifies: [DESIGN] RULES.md D385: an equality of a key to one value
/// keeps at most one row, and a join to a key at most the other side's
/// bound; a filter of no equality bounds nothing past its table.
#[test]
fn a_key_bounds_its_equality_and_its_join() {
    let (_db, s) = database();
    let (rows, upper) = card(&s, "SELECT * FROM P WHERE PK = 5");
    assert!(upper <= 1.0_f64 && rows <= 1.0_f64, "{rows} at most {upper}");
    let (_, joined) = card(&s, "SELECT * FROM PS, P WHERE PS.PK = P.PK");
    assert!((800.0_f64..=3_200.0_f64).contains(&joined), "each offer meets one part: {joined}");
    let (_, ranged) = card(&s, "SELECT * FROM P WHERE SZ < 10");
    assert!((40.0_f64..=200.0_f64).contains(&ranged), "{ranged}");
}

/// A generator of numbers from a fixed seed (xorshift64).
struct Seeded(u64);

impl Seeded {
    /// A number below `n`.
    fn next(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13_u32;
        self.0 ^= self.0 >> 7_u32;
        self.0 ^= self.0 << 17_u32;
        self.0.checked_rem(n).unwrap_or(0)
    }
}

/// Verifies: [DESIGN] RULES.md D385: on random small tables of skewed
/// values, the bound of every join and filter asked is never below the
/// rows it gives, counted exhaustively, and no estimate passes its bound.
#[test]
fn bounds_are_never_below_the_true_rows() {
    let mut random = Seeded(0x0B00_D5ED);
    for round in 0_i32..6_i32 {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION R CREATE TABLE A (X INTEGER, Y INTEGER) CREATE TABLE B (X INTEGER, Y INTEGER) CREATE TABLE C (X INTEGER, Y INTEGER)")
            .unwrap();
        let s = db.session("R");
        for table in ["A", "B", "C"] {
            let n = random.next(80).wrapping_add(1);
            let domain = random.next(12).wrapping_add(1);
            let values: Vec<String> =
                (0..n).map(|_| format!("({}, {})", random.next(domain).min(random.next(domain)), random.next(6))).collect();
            s.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        s.commit().unwrap();
        db.maintain().unwrap();
        let joins = [
            "A, B WHERE A.X = B.X",
            "A, B WHERE A.X = B.X AND A.Y = 2",
            "A, B, C WHERE A.X = B.X AND B.Y = C.Y",
            "A, B, C WHERE A.X = B.X AND B.X = C.X AND C.Y = 1",
            "A WHERE X = 3",
            "A, B WHERE A.X = B.Y AND A.Y = B.X",
        ];
        for from in joins {
            let (rows, upper) = card(&s, &format!("SELECT * FROM {from}"));
            let truth = count(&s, &format!("SELECT COUNT(*) FROM {from}"));
            assert!(upper >= truth, "round {round}, {from}: bound {upper} below {truth}");
            assert!(rows <= upper, "round {round}, {from}: {rows} past {upper}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D302, D384: with pairs of relations
/// corrected, every order of joining a set still gives it the same rows,
/// each pair's factor counted once both its relations are in.
#[test]
fn corrected_pairs_give_one_answer_for_every_order() {
    let classes =
        Classes::new(vec![vec![(0, 200.0_f64), (1, 200.0_f64)], vec![(0, 20.0_f64), (1, 20.0_f64)], vec![(1, 50.0_f64), (2, 50.0_f64)]])
            .with_pairs(vec![(0, 1, 5.0_f64)]);
    let rows = [2_000.0_f64, 800.0_f64, 50.0_f64];
    let fraction = |set: &[usize]| classes.fraction(|r| set.contains(&r));
    let joined = |order: [usize; 3]| {
        let mut got = rows[order[0]];
        let mut set = vec![order[0]];
        for &r in &order[1..] {
            let union: Vec<usize> = set.iter().copied().chain([r]).collect();
            got = got * rows[r] * fraction(&union) / (fraction(&set) * fraction(&[r]));
            set = union;
        }
        got
    };
    let all = rows.iter().product::<f64>() * fraction(&[0, 1, 2]);
    for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0], [0, 2, 1]] {
        assert!((joined(order) / all - 1.0_f64).abs() < 1e-9_f64, "{order:?}");
    }
    assert!((fraction(&[0, 1]) - 5.0_f64 / 4_000.0_f64).abs() < 1e-12_f64, "the pair's factor applies");
    assert!((fraction(&[0, 2]) - 1.0_f64).abs() < 1e-12_f64, "and not to a set of one of them");
}

/// Verifies: [DESIGN] RULES.md D385: an estimate above a bound it is
/// told is lowered to it, and the lesser bound kept; one below is kept.
#[test]
fn an_estimate_is_clamped_to_its_bound() {
    let lowered = super::Card { rows: 10.0_f64, upper: f64::INFINITY }.bounded(3.0_f64);
    assert!((lowered.rows - 3.0_f64).abs() < f64::EPSILON && (lowered.upper - 3.0_f64).abs() < f64::EPSILON);
    let kept = super::Card { rows: 2.0_f64, upper: 5.0_f64 }.bounded(8.0_f64);
    assert!((kept.rows - 2.0_f64).abs() < f64::EPSILON && (kept.upper - 5.0_f64).abs() < f64::EPSILON);
}

/// Verifies: [DESIGN] RULES.md D385: rows written since the statistics
/// pass may all hold one value, so the bound of an equality to it and of
/// a join on it stays above the rows they give; fewer rows are written
/// than call for another pass (STATISTICS.REFRESH_ROWS).
#[test]
fn a_table_changed_since_its_pass_keeps_its_bound() {
    let (_db, s) = database();
    let more: Vec<String> = (0_i32..60_i32).map(|i| format!("({}, 7)", i.wrapping_add(1_000_i32))).collect();
    s.run(&format!("INSERT INTO P VALUES {}", more.join(", "))).unwrap();
    s.commit().unwrap();
    let sevens = count(&s, "SELECT COUNT(*) FROM P WHERE SZ = 7");
    let (_, filtered) = card(&s, "SELECT * FROM P WHERE SZ = 7");
    assert!(filtered >= sevens, "bound {filtered} below {sevens}");
    let pairs = count(&s, "SELECT COUNT(*) FROM P A, P B WHERE A.SZ = B.SZ AND A.PK = 7");
    let (_, joined) = card(&s, "SELECT * FROM P A, P B WHERE A.SZ = B.SZ AND A.PK = 7");
    assert!(joined >= pairs, "bound {joined} below {pairs}");
}
