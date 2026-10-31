//! \[DESIGN\] Operators over a stream's vectors (RULES.md D31, D38): a
//! sort, EXCEPT and INTERSECT read their operands by batch, and grouping
//! past the work area writes its rows by batch; each answers as row by row
//! does (EXECUTOR.BATCHES OFF), in memory and spilled, of a table of the
//! column store, its rows frozen and changed, as of one of the row store.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};
use turborel::threads::JobPolicy;

use crate::common::{both_ways, exec, rows, sorted};
use crate::spilling::{SMALL, bytes};

/// The rows written: K, a key; G, one of 7, null every 11th; D, a
/// DECIMAL(9, 2) of either sign, null every 13th; F, a DOUBLE PRECISION,
/// null every 17th; A, a CHAR(12) of 30 values; V, a VARCHAR(16): A's with
/// a trailing space, texts longer than a sort's prefix, others, some not
/// ASCII, null every 19th.
const ROWS: i64 = 6000;

/// Row `k`, as a row value constructor.
fn row(k: i64) -> String {
    let nulled = |every: i64, made: String| if k % every == 0 { "NULL".to_owned() } else { made };
    let group = nulled(11, (k % 7).to_string());
    let decimal = nulled(13, format!("{}.{:02}", (k % 300) - 150, k % 100));
    let float = nulled(17, format!("{}E-1", (k % 50) * 5 - 100));
    let text = match k % 4 {
        0 => format!("'a{} '", k % 30),
        1 => format!("'long text {} tail'", k % 9),
        2 => format!("'v{}'", k % 40),
        _ => format!("'é{}'", k % 5),
    };
    format!("({k}, {group}, {decimal}, {float}, 'a{}', {})", k % 30, nulled(19, text))
}

/// Tables R, of the row store, and C, of the column store in groups of
/// 2048, of schema A, alike: written, C's rows frozen, then some changed,
/// deleted and added, in each.
fn both(db: &Database) -> Session {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER PRIMARY KEY, G SMALLINT, D DECIMAL(9, 2), F DOUBLE PRECISION, A CHAR(12), V VARCHAR(16))
           CREATE TABLE C (K INTEGER PRIMARY KEY, G SMALLINT, D DECIMAL(9, 2), F DOUBLE PRECISION, A CHAR(12), V VARCHAR(16))
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..ROWS).step_by(500) {
            let values: Vec<String> = (first..first + 500).map(row).collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
        db.maintain().unwrap();
        a.run(&format!("UPDATE {table} SET D = D + 0.01, V = 'changed' WHERE K BETWEEN 100 AND 160")).unwrap();
        a.run(&format!("DELETE FROM {table} WHERE K BETWEEN 3000 AND 3050")).unwrap();
        a.run(&format!("INSERT INTO {table} VALUES (-1, 1, 1.5, 0E0, 'new', 'a1'), (-2, NULL, NULL, NULL, NULL, NULL)")).unwrap();
        a.commit().unwrap();
    }
    a
}

/// Sorts, each by K last, so each order is one: `{t}` the table.
const SORTS: [&str; 9] = [
    "SELECT G, K FROM {t} ORDER BY G, K",
    "SELECT G, K FROM {t} ORDER BY G DESC, K DESC",
    "SELECT D, K FROM {t} ORDER BY D, K",
    "SELECT D, G, K FROM {t} WHERE G <> 3 ORDER BY D DESC, G, K",
    "SELECT F, K FROM {t} ORDER BY F DESC, K",
    "SELECT A, K FROM {t} ORDER BY A, K",
    "SELECT V, K FROM {t} ORDER BY V DESC, K",
    "SELECT V, A, K FROM {t} WHERE V LIKE 'long%' ORDER BY V, A, K",
    "SELECT K FROM {t} ORDER BY K DESC",
];

/// The rows of each of `queries` of table `t`, in their order, read by
/// batches where they can be and row by row: asserted alike.
fn in_order(s: &Session, queries: &[&str], t: &str) -> Vec<Vec<String>> {
    queries
        .iter()
        .map(|q| {
            let sql = q.replace("{t}", t);
            let batched = rows(s, &sql).unwrap();
            exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
            let by_row = rows(s, &sql).unwrap();
            exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
            assert_eq!(batched, by_row, "{sql}");
            batched
        })
        .collect()
}

/// Verifies: [DESIGN] RULES.md D31, D38 [X3.135-1986 §8.3 GR3b]: a sort of
/// a stream's batches gives the rows, in the order, row by row gives them,
/// of a table of each store, in memory and past a work area of its least:
/// ascending and DESC, nulls among them, of integers, decimals,
/// approximate numbers, CHARACTER and VARYING texts, padded alike, longer
/// than a prefix, filtered or not, frozen rows changed among them; the
/// areas it spilled to given back.
#[test]
fn sorts_by_batch_answer_as_row_by_row() {
    let db = Database::new();
    let a = both(&db);
    let held = in_order(&a, &SORTS, "C");
    assert_eq!(held, in_order(&a, &SORTS, "R"));
    let reserved = db.memory().unwrap().1;
    exec(&a, &format!("SET MEMORY.WORK_BYTES = {SMALL}")).unwrap();
    for t in ["C", "R"] {
        assert_eq!(in_order(&a, &SORTS, t), held, "{t}, spilled");
    }
    assert_eq!(db.memory().unwrap().1, reserved, "its areas given back");
    assert_eq!(held[0].first().map(String::as_str), Some("0,7"), "G 0 first, K 0's null");
    assert_eq!(held[0].last().map(String::as_str), Some("NULL,5995"), "nulls last");
    assert_eq!(held[5][0], "a0          ,0", "CHARACTER padded");
}

/// EXCEPT and INTERSECT, of tables `{t}` and `{o}`, their keys of either
/// store's vectors, of exact numbers of two scales, texts of two types.
const SETS: [&str; 8] = [
    "SELECT G FROM {t} EXCEPT SELECT G FROM {o} WHERE K > 3000",
    "SELECT D FROM {t} EXCEPT ALL SELECT D FROM {o} WHERE K < 2000",
    "SELECT D FROM {t} INTERSECT SELECT D + 0.000 FROM {o}",
    "SELECT A FROM {t} INTERSECT ALL SELECT V FROM {o}",
    "SELECT A FROM {t} EXCEPT SELECT V FROM {o}",
    "SELECT F, G FROM {t} EXCEPT SELECT F, G FROM {o} WHERE K < 4000",
    "SELECT K, V FROM {t} INTERSECT SELECT K, V FROM {o} WHERE V IS NULL OR K < 100",
    "SELECT G, D FROM {t} WHERE K < 500 INTERSECT ALL SELECT G, D FROM {o}",
];

/// The rows, sorted, of each of `SETS` of tables `t` and `o`, read by
/// batches where they can be and row by row: asserted alike.
fn sets_of(s: &Session, (t, o): (&str, &str)) -> Vec<Vec<String>> {
    SETS.iter()
        .map(|q| {
            let sql = q.replace("{t}", t).replace("{o}", o);
            let (batched, by_row) = both_ways(s, &sql);
            assert_eq!(batched, by_row, "{sql}");
            batched
        })
        .collect()
}

/// Verifies: [DESIGN] RULES.md D31, D38 [ISO/IEC 9075:1992 §7.10 GR1b]:
/// EXCEPT and INTERSECT, ALL or not, of operands read by batch, give the
/// rows row by row gives, in memory and partitioned past a work area of
/// its least, of tables of either store, each the other's operand: values
/// equal by comparison one row, 1.5 and 1.500, 'a1' and 'a1 ', CHARACTER
/// and VARYING, nulls identical.
#[test]
fn set_operations_by_batch_answer_as_row_by_row() {
    let db = Database::new();
    let a = both(&db);
    let pairs = [("C", "R"), ("R", "C"), ("C", "C")];
    let held: Vec<Vec<Vec<String>>> = pairs.iter().map(|&p| sets_of(&a, p)).collect();
    assert_eq!(held[0], held[1], "of either store");
    assert_eq!(held[0], held[2], "of either store");
    exec(&a, &format!("SET MEMORY.WORK_BYTES = {SMALL}")).unwrap();
    for (&p, expected) in pairs.iter().zip(&held) {
        assert_eq!(&sets_of(&a, p), expected, "{p:?}, spilled");
    }
    assert!(held[0][2].contains(&"1.500".to_owned()), "1.50 and 1.500 one row, of the scale of both");
    assert!(held[0][3].contains(&"a1          ".to_owned()), "padded alike");
    assert!(held[0][6].iter().any(|r| r.ends_with(",NULL")), "nulls identical");
}

/// Groupings of more groups than the least work area holds.
const GROUPS: [&str; 4] = [
    "SELECT K, COUNT(*), SUM(D), MIN(V), MAX(F) FROM {t} GROUP BY K",
    "SELECT D, G, COUNT(*), SUM(K + 1), AVG(F) FROM {t} GROUP BY D, G",
    "SELECT V, A, COUNT(D) FROM {t} GROUP BY V, A",
    "SELECT F, K, MAX(A) FROM {t} WHERE G <> 2 GROUP BY F, K",
];

/// Verifies: [DESIGN] RULES.md D31, D38 [X3.135-1986 §5.22, §5.8]
/// [ISO/IEC 9075:1992 §6.5 GR1b]: groups past a work area of its least,
/// their rows written by batch and grouped again by batch, give what row
/// by row gives, in the order the groups held in memory come, whatever
/// SPILL.PARTITIONS and SPILL.PARTITION_DEPTH: partitioned again, and made
/// in passes; grouped by integers, decimals, approximate numbers and
/// texts, nulls a group; a null a set function takes out warns 01003 as
/// row by row; the areas spilled to given back.
#[test]
fn groups_past_the_work_area_by_batch_answer_as_row_by_row() {
    let db = Database::new();
    let a = both(&db);
    let held: Vec<Vec<String>> = GROUPS.iter().map(|q| rows(&a, &q.replace("{t}", "C")).unwrap()).collect();
    let reserved = db.memory().unwrap().1;
    exec(&a, &format!("SET MEMORY.WORK_BYTES = {SMALL}")).unwrap();
    for (partitions, depth) in [(64, 8), (2, 8), (2, 0)] {
        db.set_setting("SPILL.PARTITIONS", &bytes(partitions)).unwrap();
        db.set_setting("SPILL.PARTITION_DEPTH", &bytes(depth)).unwrap();
        for (q, expected) in GROUPS.iter().zip(&held) {
            let sql = q.replace("{t}", "C");
            assert_eq!(&rows(&a, &sql).unwrap(), expected, "{sql}: in the order held in memory");
            for t in ["C", "R"] {
                let sql = q.replace("{t}", t);
                let (batched, by_row) = both_ways(&a, &sql);
                assert_eq!(batched, by_row, "{sql}, {partitions} partitions, depth {depth}");
            }
        }
        assert_eq!(db.memory().unwrap().1, reserved, "its areas given back");
    }
    for batches in ["OFF", "ON"] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        assert_eq!(rows(&a, &GROUPS[0].replace("{t}", "C")).map(|r| r.len()), Ok(5951));
        assert_eq!(a.warning(), Some(SqlState::NULL_VALUE_ELIMINATED), "{batches}");
    }
    assert_eq!(sorted(&a, &GROUPS[2].replace("{t}", "C")), sorted(&a, &GROUPS[2].replace("{t}", "R")));
}

/// Verifies: [DESIGN] RULES.md D15, D31: an error raised of a stream's
/// rows under a sort, EXCEPT or INTERSECT read by batch, is the one row by
/// row raises: of a filter, and of a projection.
#[test]
fn errors_under_vector_operators_are_raised_as_row_by_row() {
    let db = Database::new();
    let a = both(&db);
    let raising = [
        "SELECT K FROM {t} WHERE 10 / (K - 4321) > 0 ORDER BY K",
        "SELECT 10 / (K - 4321) FROM {t} ORDER BY 1",
        "SELECT K FROM {t} EXCEPT SELECT K FROM {t} WHERE 10 / (K - 4321) > 0",
        "SELECT 10 / (K - 4321) FROM {t} INTERSECT SELECT K FROM {t}",
    ];
    for q in raising {
        for t in ["C", "R"] {
            let sql = q.replace("{t}", t);
            let batched = rows(&a, &sql).err();
            exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
            assert_eq!(rows(&a, &sql).err(), batched, "{sql}");
            exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
            assert_eq!(batched, Some(Code::Data), "{sql}");
        }
    }
}
