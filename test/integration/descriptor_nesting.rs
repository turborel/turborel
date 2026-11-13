//! Subordinate SQL item descriptor areas [ISO/IEC 9075-2:2023 §20.1 SR4,
//! SR5, §20.4, §20.5, §20.9 GR7, §20.11, §20.12]: a row's fields and an
//! array's element described WITH NESTING, LEVEL, DEGREE and CARDINALITY
//! got and set, and the values of rows and arrays read INTO and written
//! USING a descriptor through them.

use std::sync::Arc;

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::value::types::Field;
use turborel::value::{DataType, Decimal, Spelled, Value};

use crate::common::{dynamic_host, rows, run_with as run, set_text as text};

/// INTEGER.
fn integer() -> DataType {
    DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer }
}

/// The whole number `n`.
fn whole(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Schema APP: table T of an INTEGER K, R ROW(X INTEGER, Y VARCHAR(5))
/// and A INTEGER ARRAY[3], holding (1, (2, 'b'), [3, 4]).
fn nested() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER, R ROW(X INTEGER, Y VARCHAR(5)), A INTEGER ARRAY[3])").unwrap();
    let s = db.session("APP");
    s.run("INSERT INTO T VALUES (1, ROW(2, 'b'), ARRAY[3, 4])").unwrap();
    (db, s)
}

/// `sql` prepared as `name` and described into descriptor `descriptor`
/// with its `nesting`, OUTPUT or INPUT as `output` says.
fn described(s: &Session, (sql, name): (&str, &str), descriptor: &str, nesting: &str, output: bool) {
    let mut p = dynamic_host();
    text(&mut p, sql);
    run(s, &format!("PREPARE {name} FROM TEXT"), &mut p).unwrap();
    run(s, &format!("ALLOCATE DESCRIPTOR '{descriptor}'"), &mut p).unwrap();
    let kind = if output { "OUTPUT" } else { "INPUT" };
    run(s, &format!("DESCRIBE {kind} {name} USING SQL DESCRIPTOR '{descriptor}' {nesting}"), &mut p).unwrap();
}

/// The header's COUNT and TOP_LEVEL_COUNT of descriptor D.
fn counts(s: &Session) -> (String, String) {
    let mut p = dynamic_host();
    run(s, "GET DESCRIPTOR 'D' N = COUNT, VAL = TOP_LEVEL_COUNT", &mut p).unwrap();
    (p.value("N").to_string(), p.value("VAL").to_string())
}

/// LEVEL, TYPE, NAME, UNNAMED, DEGREE, CARDINALITY and NULLABLE of each of
/// the first `count` items of descriptor D.
fn items(s: &Session, count: i64) -> Vec<String> {
    let mut p = dynamic_host();
    let mut out = Vec::new();
    for n in 1..=count {
        let get = format!("GET DESCRIPTOR 'D' VALUE {n} TXT = NAME, N = LEVEL, VAL = TYPE, ID = UNNAMED");
        run(s, &get, &mut p).unwrap();
        let head = [p.value("N"), p.value("VAL"), p.value("TXT"), p.value("ID")].map(|v| v.to_string().trim_end().to_owned());
        run(s, &format!("GET DESCRIPTOR 'D' VALUE {n} N = DEGREE, VAL = CARDINALITY, ID = NULLABLE"), &mut p).unwrap();
        let tail = [p.value("N"), p.value("VAL"), p.value("ID")].map(|v| v.to_string());
        out.push([head.join("|"), tail.join("|")].join("|"));
    }
    out
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.9 GR7, §20.1 SR4, SR5, Table 30]
/// DESCRIBE WITH NESTING gives each item its subordinate items in
/// preorder: a row's fields, of LEVEL one more, named, and an array's
/// element, unnamed; TYPE ROW 19 with DEGREE its fields, ARRAY 50 with
/// CARDINALITY its maximum cardinality; COUNT all, TOP_LEVEL_COUNT those
/// of LEVEL 0. WITHOUT NESTING, as when neither is written, gives the
/// top level alone.
#[test]
fn described_with_nesting_20_9() {
    let (_db, s) = nested();
    described(&s, ("SELECT K, R, A FROM T", "Q"), "D", "WITH NESTING", true);
    assert_eq!(counts(&s), ("6".to_owned(), "3".to_owned()));
    assert_eq!(items(&s, 6), ["0|4|K|0|0|0|1", "0|19|R|0|2|0|1", "1|4|X|0|0|0|1", "1|12|Y|0|0|0|1", "0|50|A|0|0|3|1", "1|4||1|0|0|1"]);
    let mut p = dynamic_host();
    run(&s, "DESCRIBE OUTPUT Q USING SQL DESCRIPTOR 'D' WITHOUT NESTING", &mut p).unwrap();
    assert_eq!(counts(&s), ("3".to_owned(), "3".to_owned()));
    assert_eq!(items(&s, 3), ["0|4|K|0|0|0|1", "0|19|R|0|2|0|1", "0|50|A|0|0|3|1"]);
    run(&s, "DESCRIBE OUTPUT Q USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    assert_eq!(counts(&s), ("3".to_owned(), "3".to_owned()), "WITHOUT NESTING is implicit");
    assert_eq!(run(&s, "SET DESCRIPTOR 'D' TOP_LEVEL_COUNT = 1", &mut p), Err("2A000"), "§20.5 SR2");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.9 GR7c iii 2 A, §9.5 SR3g] a field's
/// item is named as its field, UNNAMED 1 if the name is implementation-
/// dependent (UV077): of a row made of values, or one of fields named
/// differently brought together by UNION; UNNAMED 0 if declared.
#[test]
fn field_names_described_20_9() {
    let (_db, s) = nested();
    described(&s, ("SELECT ROW(K, 'z'), R FROM T UNION SELECT ROW(1, 'a'), R FROM T", "Q"), "D", "WITH NESTING", true);
    let names = items(&s, 6).iter().map(|i| i.split('|').skip(2).take(2).collect::<Vec<_>>().join("|")).collect::<Vec<_>>();
    assert_eq!(names, ["|1", "F1|1", "F2|1", "R|0", "X|0", "Y|0"]);
    let mut p = dynamic_host();
    text(&mut p, "SELECT R FROM T UNION SELECT ROW(1, 'a') FROM T");
    run(&s, "PREPARE U FROM TEXT", &mut p).unwrap();
    run(&s, "DESCRIBE OUTPUT U USING SQL DESCRIPTOR 'D' WITH NESTING", &mut p).unwrap();
    let names = items(&s, 3).iter().map(|i| i.split('|').skip(2).take(2).collect::<Vec<_>>().join("|")).collect::<Vec<_>>();
    assert_eq!(names, ["|1", "F1|1", "F2|1"], "SR3g: X and F1 differ");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.12 GR, §20.4 GR4, GR5] EXECUTE INTO
/// a descriptor described WITH NESTING sets each field's item, and the
/// array's DATA whole with RETURNED_CARDINALITY its elements; GET of the
/// DATA of a ROW item, or of an array's element, is 0700C, a target whose
/// type does not match first 22005.
#[test]
fn read_into_subordinate_items_20_12() {
    let (_db, s) = nested();
    described(&s, ("SELECT K, R, A FROM T", "Q"), "D", "WITH NESTING", true);
    let row = DataType::row(vec![
        Field { name: "X".into(), data_type: integer(), unnamed: false },
        Field { name: "Y".into(), data_type: DataType::varying(5), unnamed: false },
    ]);
    let mut p = dynamic_host()
        .declare("Y5", DataType::varying(5), Value::Null)
        .declare("ARR", DataType::array(integer(), 3), Value::Null)
        .declare("RV", row, Value::Null);
    run(&s, "EXECUTE Q INTO SQL DESCRIPTOR 'D'", &mut p).unwrap();
    run(&s, "GET DESCRIPTOR 'D' VALUE 3 N = DATA", &mut p).unwrap();
    run(&s, "GET DESCRIPTOR 'D' VALUE 4 Y5 = DATA", &mut p).unwrap();
    run(&s, "GET DESCRIPTOR 'D' VALUE 5 ARR = DATA, VAL = RETURNED_CARDINALITY", &mut p).unwrap();
    let got = ["N", "Y5", "ARR", "VAL"].map(|n| p.value(n).to_string());
    assert_eq!(got, ["2", "b", "ARRAY[3, 4]", "2"]);
    assert_eq!(run(&s, "GET DESCRIPTOR 'D' VALUE 2 RV = DATA", &mut p), Err("0700C"), "GR5b");
    assert_eq!(run(&s, "GET DESCRIPTOR 'D' VALUE 6 N = DATA", &mut p), Err("0700C"), "GR5a");
    assert_eq!(run(&s, "GET DESCRIPTOR 'D' VALUE 2 N = DATA", &mut p), Err("22005"), "GR4");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.5 GR4b] LEVEL is set only as the
/// items before it allow. MC/DC of each rule: the first item's LEVEL 1
/// against 0 [i]; one more than the item before's after an INTEGER
/// against after a ROW [ii 1]; two more [ii 2]; less, closing a row of
/// DEGREE 2 after one field against after both [ii 3 A]; a field of a row
/// all of whose fields are before it [ii 3 B]. Each refusal is 0700E.
#[test]
fn level_set_20_5() {
    let (_db, s) = nested();
    described(&s, ("SELECT K, R, A FROM T", "Q"), "D", "WITH NESTING", true);
    let mut p = dynamic_host();
    let mut set = |item: i64, level: i64| run(&s, &format!("SET DESCRIPTOR 'D' VALUE {item} LEVEL = {level}"), &mut p).map(|_| ());
    assert_eq!(set(1, 1), Err("0700E"), "i");
    assert_eq!(set(1, 0), Ok(()), "i");
    assert_eq!(set(2, 1), Err("0700E"), "ii 1: after an INTEGER");
    assert_eq!(set(3, 1), Ok(()), "ii 1: after a ROW");
    assert_eq!(set(3, 2), Err("0700E"), "ii 2");
    assert_eq!(set(4, 0), Err("0700E"), "ii 3 A: R has one of its two fields");
    assert_eq!(set(5, 0), Ok(()), "ii 3 A: R has both");
    assert_eq!(set(5, 1), Err("0700E"), "ii 3 B: R has no third field");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.5 GR4a, §20.11 GR, §20.1 SR7] SET
/// of the DATA of a ROW item, or of an array's element, is 0700D; a
/// field's DATA must match its type (22005). EXECUTE USING a descriptor
/// described WITH NESTING makes each row of its fields' items, null if
/// its INDICATOR is negative, and each array of its DATA; one whose TYPE
/// was set since, its DATA undefined, is 07001.
#[test]
fn written_using_subordinate_items_20_11() {
    let (_db, s) = nested();
    described(&s, ("INSERT INTO T VALUES (?, ?, ?)", "I"), "E", "WITH NESTING", false);
    let seven = Value::Array(Arc::from(vec![whole(7)]));
    let mut p: Params = dynamic_host()
        .declare("ARR", DataType::array(integer(), 3), seven)
        .declare("V5", DataType::varying(5), Value::Char("yy".into()))
        .declare("C2", DataType::character(2), Value::Char("zz".into()));
    p.set("K", whole(5));
    for sql in [
        "SET DESCRIPTOR 'E' VALUE 1 DATA = K",
        "SET DESCRIPTOR 'E' VALUE 3 DATA = K",
        "SET DESCRIPTOR 'E' VALUE 4 DATA = V5",
        "SET DESCRIPTOR 'E' VALUE 5 DATA = ARR",
        "EXECUTE I USING SQL DESCRIPTOR 'E'",
    ] {
        assert!(run(&s, sql, &mut p).is_ok(), "{sql}");
    }
    assert_eq!(run(&s, "SET DESCRIPTOR 'E' VALUE 2 DATA = K", &mut p), Err("0700D"), "GR4a ii");
    assert_eq!(run(&s, "SET DESCRIPTOR 'E' VALUE 6 DATA = K", &mut p), Err("0700D"), "GR4a i");
    assert_eq!(run(&s, "SET DESCRIPTOR 'E' VALUE 4 DATA = C2", &mut p), Err("22005"), "GR4a iii");
    run(&s, "SET DESCRIPTOR 'E' VALUE 2 INDICATOR = -1", &mut p).unwrap();
    run(&s, "EXECUTE I USING SQL DESCRIPTOR 'E'", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'E' VALUE 2 TYPE = 4", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE I USING SQL DESCRIPTOR 'E'", &mut p), Err("07001"));
    let kept = rows(&s, "SELECT K, R, A FROM T WHERE K = 5").unwrap();
    assert_eq!(kept, ["5,ROW(5, 'yy'),ARRAY[7]", "5,NULL,ARRAY[7]"]);
}
