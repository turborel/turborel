//! Unit tests: a dynamic cursor's query keeps its specification and plan
//! between its OPENs (RULES.md D45, R-TEST-1), as the plans its statement
//! made tell, which SQL text does not show.

use super::super::Name;
use crate::db::{Completion, Database, Session};
use crate::error::Code;
use crate::params::Params;
use crate::value::{DataType, Decimal, Spelled, Value};

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// How many plans the statement prepared as `name` in `s` made.
fn made(s: &Session, name: &str) -> u64 {
    s.prepared_as(&Name::plain(name), "").unwrap().plans_made()
}

/// Schema A with T (K INTEGER NOT NULL, N INTEGER) of rows K 1 to 5, N 0;
/// a session of A, with the statement prepared as S, of `text`, and a
/// cursor C declared for it, its values the parameter V, its rows fetched
/// into FK.
fn cursor_for(text: &str) -> (Database, Session, Params) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL, N INTEGER)").unwrap();
    let s = db.session("A");
    for k in 1_i32..=5_i32 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 0)")).unwrap();
    }
    s.commit().unwrap();
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    let mut p = Params::new()
        .declare("TEXT", DataType::Varying { length: 100 }, Value::Char(text.to_owned()))
        .declare("V", integer, int(0))
        .declare("FK", integer, int(0));
    s.execute("PREPARE S FROM TEXT", &mut p).unwrap();
    s.declare_cursor("DECLARE C CURSOR FOR S").unwrap();
    (db, s, p)
}

/// The first row C gives opened with `v`, closed after.
fn first(s: &Session, p: &mut Params, v: i128) -> String {
    p.set("V", int(v));
    s.execute("OPEN C USING V", p).unwrap();
    assert_eq!(s.execute("FETCH C INTO FK", p).unwrap(), Completion::Success);
    s.execute("CLOSE C", p).unwrap();
    p.value("FK").to_string()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.14 GR5, GR6, §17.6 GR10];
/// [DESIGN] RULES.md D45, D58: a dynamic cursor's query, read-only, opened
/// again with other values, is bound once while what it was bound with
/// holds, its rows those of its values; bound again once an index is
/// made; refused once its table is dropped. Regression test (R-TEST-7):
/// it was bound at each OPEN.
#[test]
fn a_dynamic_cursors_query_keeps_its_plan_between_opens() {
    let (db, s, mut p) = cursor_for("SELECT K FROM T WHERE K >= ? ORDER BY K");
    for v in 1..=3 {
        assert_eq!(first(&s, &mut p, v), v.to_string());
    }
    assert_eq!(made(&s, "S"), 1);
    db.create_index("T_K", "A.T", &["K"], "btree").unwrap();
    assert_eq!(first(&s, &mut p, 4), "4");
    assert_eq!(first(&s, &mut p, 5), "5");
    assert_eq!(made(&s, "S"), 2, "an index made");
    s.commit().unwrap();
    s.run("DROP TABLE T CASCADE").unwrap();
    assert_eq!(s.execute("OPEN C USING V", &mut p).err().map(|e| e.code), Some(Code::SyntaxRule), "T is no more");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.1 SR6, §17.14, §17.20];
/// [DESIGN] RULES.md D45, D58: a dynamic cursor's query, updatable, keeps
/// its plan between OPENs, as a positioned UPDATE prepared through it
/// does, each changing the row the cursor is on.
#[test]
fn an_updatable_dynamic_cursor_keeps_its_plan_between_opens() {
    let (_db, s, mut p) = cursor_for("SELECT K FROM T WHERE K >= ?");
    p.set("TEXT", Value::Char("UPDATE T SET N = ? WHERE CURRENT OF C".to_owned()));
    s.execute("PREPARE U FROM TEXT", &mut p).unwrap();
    for v in 1..=3 {
        let k = first(&s, &mut p, v);
        p.set("V", int(v));
        s.execute("OPEN C USING V", &mut p).unwrap();
        s.execute("FETCH C INTO FK", &mut p).unwrap();
        s.execute("EXECUTE U USING V", &mut p).unwrap();
        s.execute("CLOSE C", &mut p).unwrap();
        assert_eq!(p.value("FK").to_string(), k);
    }
    s.commit().unwrap();
    assert_eq!([made(&s, "S"), made(&s, "U")], [1, 1]);
    let mut changed = s.open_cursor("SELECT SUM(N) FROM T WHERE N > 0").unwrap();
    assert_eq!(changed.fetch().unwrap().map(|r| r[0].to_string()), Some("6".to_owned()));
}
