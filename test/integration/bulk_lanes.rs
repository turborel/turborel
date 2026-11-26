//! \[DESIGN\] A load given by column, as text (RULES.md D1148): parsed
//! straight into each column's type and written as a group, its table
//! then as a load of the same rows given as rows of text leaves it; and,
//! where the rows break a constraint or a cell is no value of its type,
//! refused as they are.

use std::sync::Arc;

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Load;
use turborel::storage::lanes::{Lane, Lanes};
use turborel::threads::JobPolicy;
use turborel::value::{CharString, Value};

use crate::common::rows;

/// The rows of a load.
const ROWS: usize = 9000;

/// T, of the column store in groups of 4096 rows: a key, an amount, a
/// name, a day, a column the copy leaves to its default, and a CHECK.
const TABLE: &str = "CREATE TABLE T (K INTEGER NOT NULL, AMOUNT NUMERIC(10, 2), NAME VARCHAR(12), DUE DATE, \
                     KEPT INTEGER DEFAULT 7, CHECK (AMOUNT >= 0)) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 4096)";

/// The INSERT of a copy into T's columns but KEPT.
const INSERT: &str = "INSERT INTO T (K, AMOUNT, NAME, DUE) VALUES (?, ?, ?, ?)";

/// A database of S with T, its background work done only when asked.
fn database() -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION S {TABLE}")).unwrap();
    db
}

/// The text of each cell of row `k`, none for a null.
fn cells(k: usize) -> [Option<String>; 4] {
    let amount = (k % 11 != 4).then(|| format!("{}.{:02}", k * 3 % 1000, k % 100));
    let name = (k % 13 != 6).then(|| format!("name {}", k % 97));
    let day = Some(format!("20{:02}-{:02}-{:02}", k % 30, k % 12 + 1, k % 28 + 1));
    [Some(k.to_string()), amount, name, day]
}

/// The lanes of `rows`, each row's cells.
fn lanes_of(rows: &[[Option<String>; 4]]) -> Lanes {
    let mut columns = vec![Lane { nulls: vec![0; rows.len().div_ceil(8)], ..Lane::default() }; 4];
    for (n, row) in rows.iter().enumerate() {
        for (lane, cell) in columns.iter_mut().zip(row) {
            match cell {
                Some(text) => lane.bytes.extend_from_slice(text.as_bytes()),
                None => lane.nulls[n >> 3] |= 1 << (n & 7),
            }
            lane.ends.push(u32::try_from(lane.bytes.len()).unwrap());
        }
    }
    Lanes { rows: rows.len(), columns }
}

/// The same rows as rows of text, as a copy of rows sends them.
fn texts_of(rows: &[[Option<String>; 4]]) -> Vec<Vec<Value>> {
    let text = |c: &Option<String>| c.as_ref().map_or(Value::Null, |s| Value::Char(CharString::from(s.clone())));
    rows.iter().map(|row| row.iter().map(text).collect()).collect()
}

/// T as a session finds it, every row in order, and whether every row is
/// in a group.
fn table(s: &Session) -> (Vec<String>, bool) {
    let all = rows(s, "SELECT K, AMOUNT, NAME, DUE, KEPT FROM T ORDER BY K").unwrap();
    let in_tree = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.COLUMN_STORAGE WHERE TABLE_NAME = 'T' AND ENCODING = 'ROW'";
    (all, rows(s, in_tree).unwrap() == ["0"])
}

/// Verifies: [DESIGN] RULES.md D1148, D1147: rows given by column as text
/// go straight to groups, each column parsed into its type, the column
/// the INSERT leaves its default, its nulls nulls; T is then as the same
/// rows given as rows of text leave it, and as a load in parts of any
/// size leaves it.
#[test]
fn a_load_by_column_is_a_load_of_its_rows() {
    let all: Vec<_> = (0..ROWS).map(cells).collect();
    let by_rows = database();
    let r = by_rows.session("S");
    let p = r.prepare(INSERT).unwrap();
    r.execute_prepared_rows(&p, &texts_of(&all), Load::Going).unwrap();
    r.execute_prepared_rows(&p, &[], Load::Ending).unwrap();
    r.commit().unwrap();
    let expected = table(&r);
    assert!(expected.1 && expected.0.len() == ROWS);
    let by_column = database();
    let c = by_column.session("S");
    let p = c.prepare(INSERT).unwrap();
    for part in all.chunks(4096) {
        c.execute_prepared_lanes(&p, Arc::new(lanes_of(part)), Load::Going).unwrap();
    }
    c.execute_prepared_lanes(&p, Arc::new(Lanes::default()), Load::Ending).unwrap();
    c.commit().unwrap();
    assert_eq!(table(&c), expected);
    assert_eq!(rows(&c, "SELECT COUNT(*) FROM T WHERE AMOUNT IS NULL").unwrap(), [((0..ROWS).filter(|k| k % 11 == 4).count()).to_string()]);
}

/// Verifies: [X3.135-1986 §6.3 GR1]; [ISO/IEC 9075:1992 §11.6 GR3,
/// §13.8 GR4b iii]; [DESIGN] RULES.md D1148: rows given by column that
/// break T's NOT NULL constraint or its CHECK, or a cell no value of its
/// column's type or too long for it, are refused, as rows they are, and
/// leave T as it was. Each part is of fewer rows than a group, so it
/// would go by column.
#[test]
fn a_load_by_column_that_breaks_a_rule_is_refused() {
    let broken = |n: usize, cell: usize, text: Option<&str>| {
        let mut part: Vec<_> = (0..4000).map(cells).collect();
        part[n][cell] = text.map(str::to_owned);
        part
    };
    let cases = [
        (broken(17, 0, None), Code::Constraint),
        (broken(3900, 1, Some("-1.00")), Code::Constraint),
        (broken(9, 0, Some("ten")), Code::Data),
        (broken(3000, 2, Some("a name far too long")), Code::Api),
    ];
    for (part, code) in cases {
        let db = database();
        let s = db.session("S");
        let p = s.prepare(INSERT).unwrap();
        let e = s.execute_prepared_lanes(&p, Arc::new(lanes_of(&part)), Load::Going).unwrap_err();
        assert_eq!(e.code, code, "{e}");
        s.rollback().unwrap();
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM T").unwrap(), ["0"]);
    }
}
