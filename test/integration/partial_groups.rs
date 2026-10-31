//! \[DESIGN\] Groups made partly before they spill (RULES.md D31, D38,
//! D53): a grouping by batch past its work area, whose set functions all
//! write their states (`Accumulator::state`), writes groups partly made
//! and merges them as it reads them back; it answers as one that writes
//! its rows, as one of a set function of no state does, and as row by row,
//! whatever the spill settings, of either store.

use turborel::db::{Database, Session};
use turborel::error::{Code, Result, SqlState};
use turborel::functions::{Accumulator, AggregateFunction};
use turborel::value::{DataType, Decimal, MAX_EXACT_PRECISION, Spelled, Value};

use crate::common::{both_ways, exec, function_calls, rows, sorted};
use crate::spilling::{SMALL, bytes};
use crate::vector_operators::both;

/// TALLY, a plug-in's COUNT of the values given, whose accumulator writes
/// no state: a grouping of it writes its rows past the work area.
struct Tally;

/// TALLY's count so far.
struct Tallied(i128);

impl AggregateFunction for Tally {
    fn name(&self) -> &'static str {
        "TALLY"
    }
    fn result_type(&self, _arg: Option<DataType>) -> Result<DataType> {
        Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric })
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(Tallied(0))
    }
}

impl Accumulator for Tallied {
    fn update(&mut self, _v: &Value) -> Result<()> {
        self.0 += 1;
        Ok(())
    }
    fn finish(&self) -> Result<Value> {
        Ok(Value::Exact(Decimal::new(self.0, 0)))
    }
}

/// Rows of `REPEATING`: K, a key; C, its tenth, so each of its keys
/// comes in 10 rows together; M, of the first half one of 40, of the
/// second its own, so a grouping by it fills its work area only mid-way;
/// G, one of 7, null every 11th; D, a decimal, null every 13th; V, of 9
/// texts, a third of them with a trailing space, so MIN and MAX tie, null
/// every 19th.
const REPEATING: i64 = 6000;

/// Tables R, of the row store, and C, of the column store, of schema A,
/// alike, of `REPEATING` rows: written, C's rows frozen, then some changed
/// and deleted in each.
fn repeating(db: &Database) -> Session {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER, C INTEGER, M INTEGER, G SMALLINT, D DECIMAL(9, 2), V VARCHAR(16))
           CREATE TABLE C (K INTEGER, C INTEGER, M INTEGER, G SMALLINT, D DECIMAL(9, 2), V VARCHAR(16)) USING columnstore",
    )
    .unwrap();
    let a = db.session("A");
    let row = |k: i64| {
        let nulled = |every: i64, made: String| if k % every == 0 { "NULL".to_owned() } else { made };
        let m = if k < REPEATING.checked_div(2).unwrap() { k % 40 } else { k };
        let text = nulled(19, format!("'v{}{}'", k % 9, if k % 3 == 0 { " " } else { "" }));
        format!(
            "({k}, {}, {m}, {}, {}, {text})",
            k.checked_div(10).unwrap(),
            nulled(11, (k % 7).to_string()),
            nulled(13, format!("{}.{:02}", k % 300, k % 100))
        )
    };
    for t in ["R", "C"] {
        for first in (0..REPEATING).step_by(500) {
            let values: Vec<String> = (first..first + 500).map(row).collect();
            a.run(&format!("INSERT INTO {t} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
        db.maintain().unwrap();
        a.run(&format!("UPDATE {t} SET D = D + 0.01, V = 'changed' WHERE K BETWEEN 100 AND 160")).unwrap();
        a.run(&format!("DELETE FROM {t} WHERE K BETWEEN 3000 AND 3050")).unwrap();
        a.commit().unwrap();
    }
    a
}

/// Groupings of more groups than the least work area holds, of set
/// functions that write their states: of keys that repeat, of keys that
/// repeat only before the work area fills, and of keys each its own:
/// `{t}` the table. Each, with `, TALLY(K)` added, writes its rows.
const PARTIAL: [&str; 6] = [
    "SELECT C, COUNT(*), COUNT(V), MIN(V), MAX(D) FROM {t} GROUP BY C",
    "SELECT M, SUM(D), AVG(G), SUM(K + 1), MAX(V) FROM {t} GROUP BY M",
    "SELECT C, V, MIN(D), MAX(V) FROM {t} GROUP BY C, V",
    "SELECT M FROM {t} GROUP BY M",
    "SELECT K, COUNT(*), MIN(V) FROM {t} GROUP BY K",
    "SELECT G, M, MAX(K), MIN(V) FROM {t} WHERE K > 100 GROUP BY G, M",
];

/// The rows of `sql`, in their order, each but its last column.
fn but_last(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap().iter().map(|r| r.rsplit_once(',').map_or(r.as_str(), |p| p.0).to_owned()).collect()
}

/// Verifies: [DESIGN] RULES.md D31, D38, D53 [X3.135-1986 §5.22, §5.8]
/// [ISO/IEC 9075:1992 §6.5 GR1b]: past a work area of its least, groups
/// made partly before they spill and merged as read back give the rows,
/// in the order, that groups held in memory give, those a grouping that
/// writes its rows gives (one of TALLY, of no state), and row by row,
/// whatever SPILL.PARTITIONS and SPILL.PARTITION_DEPTH: partitioned again,
/// and made in passes; of either store; of keys that repeat, of keys that
/// repeat until the work area fills mid-way, then do not, and of keys
/// that never do; MIN and MAX of ties keep the value first given; a null
/// a set function takes out warns 01003 alike; the areas spilled to given
/// back.
#[test]
fn groups_made_partly_answer_as_their_rows() {
    let db = Database::new();
    db.register_aggregate_function(Box::new(Tally)).unwrap();
    let a = repeating(&db);
    function_calls(&a);
    let held: Vec<Vec<String>> = PARTIAL.iter().map(|q| rows(&a, &q.replace("{t}", "C")).unwrap()).collect();
    let reserved = db.memory().unwrap().1;
    exec(&a, &format!("SET MEMORY.WORK_BYTES = {SMALL}")).unwrap();
    for (partitions, depth) in [(64, 8), (2, 8), (2, 0)] {
        db.set_setting("SPILL.PARTITIONS", &bytes(partitions)).unwrap();
        db.set_setting("SPILL.PARTITION_DEPTH", &bytes(depth)).unwrap();
        for (q, expected) in PARTIAL.iter().zip(&held) {
            for t in ["C", "R"] {
                let sql = q.replace("{t}", t);
                let tallied = sql.replacen(" FROM ", ", TALLY(K) FROM ", 1);
                if t == "C" {
                    assert_eq!(&rows(&a, &sql).unwrap(), expected, "{sql}: as held in memory");
                    assert_eq!(&but_last(&a, &tallied), expected, "{sql}: as its rows written");
                }
                let (batched, by_row) = both_ways(&a, &sql);
                assert_eq!(batched, by_row, "{sql}, {partitions} partitions, depth {depth}");
            }
        }
        assert_eq!(db.memory().unwrap().1, reserved, "its areas given back");
    }
    for batches in ["OFF", "ON"] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        assert_eq!(rows(&a, &PARTIAL[0].replace("{t}", "C")).map(|r| r.len()), Ok(595));
        assert_eq!(a.warning(), Some(SqlState::NULL_VALUE_ELIMINATED), "{batches}");
    }
    assert_eq!(sorted(&a, &PARTIAL[1].replace("{t}", "C")), sorted(&a, &PARTIAL[1].replace("{t}", "R")));
}

/// Verifies: [DESIGN] RULES.md D15, D38: an error a set function's
/// argument raises of one row is raised whether its group is made partly
/// before it spills or not, by batch and row by row.
#[test]
fn an_error_of_a_group_made_partly_is_raised() {
    let db = Database::new();
    let a = both(&db);
    exec(&a, &format!("SET MEMORY.WORK_BYTES = {SMALL}")).unwrap();
    for t in ["C", "R"] {
        let sql = format!("SELECT K, MIN(10 / (K - 4321)), COUNT(*) FROM {t} GROUP BY K");
        let batched = rows(&a, &sql).err();
        exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
        assert_eq!(rows(&a, &sql).err(), batched, "{sql}");
        exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
        assert_eq!(batched, Some(Code::Data), "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md D15, D38, D53 [ISO/IEC 9075:1992 §6.5 GR2b
/// iv]: a SUM of groups made partly is judged on its total alone, never
/// on a part merged: of keys each of 10^18 - 1, 1, -1, 1 and -1, each
/// key's rows among those of 99 others, every sum is 10^18 - 1, by batch
/// and row by row, of either store; of a key whose total is past 18
/// digits, 22003 alike.
#[test]
fn a_sum_of_groups_made_partly_is_judged_on_its_total() {
    const KEYS: i64 = 3000;
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER, N NUMERIC(18))
           CREATE TABLE C (K INTEGER, N NUMERIC(18)) USING columnstore",
    )
    .unwrap();
    let a = db.session("A");
    for block in (0..KEYS).step_by(100) {
        let mut values = Vec::new();
        for n in ["999999999999999999", "1", "-1", "1", "-1"] {
            values.extend((block..block + 100).map(|k| format!("({k}, {n})")));
        }
        for t in ["R", "C"] {
            a.run(&format!("INSERT INTO {t} VALUES {}", values.join(", "))).unwrap();
        }
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    exec(&a, &format!("SET MEMORY.WORK_BYTES = {SMALL}")).unwrap();
    for t in ["C", "R"] {
        let sql = format!("SELECT K, SUM(N), COUNT(*) FROM {t} GROUP BY K");
        let (batched, by_row) = both_ways(&a, &sql);
        assert_eq!(batched, by_row, "{sql}");
        assert_eq!(batched.len(), 3000_usize);
        assert!(batched.iter().all(|r| r.contains(",999999999999999999,")), "{sql}");
    }
    a.run("INSERT INTO R VALUES (7, 999999999999999999)").unwrap();
    a.run("INSERT INTO C VALUES (7, 999999999999999999)").unwrap();
    for t in ["C", "R"] {
        let sql = format!("SELECT K, SUM(N) FROM {t} GROUP BY K");
        let batched = rows(&a, &sql).err();
        exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
        assert_eq!(rows(&a, &sql).err(), batched, "{sql}");
        exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
        assert_eq!(batched, Some(Code::Data), "{sql}");
    }
}
