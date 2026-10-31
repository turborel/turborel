//! SQL descriptor areas [ISO/IEC 9075:1992 §17.1-§17.5, §17.8, §17.9]:
//! ALLOCATE, DEALLOCATE, GET and SET DESCRIPTOR, DESCRIBE's filling of one
//! by the codes of Tables 18 to 20, and EXECUTE's values of one, and into
//! one (RULES.md D58).

use turborel::db::{Completion, Session};
use turborel::error::{Code, SqlState};
use turborel::params::Params;
use turborel::value::Value;

use crate::common::{code_with as code, dynamic_host, number, rows, run_with as run, set_text as text, setup};

/// The numeric field `field` of item `n` of descriptor area 'D'.
fn field(s: &Session, n: i64, field: &str) -> String {
    let mut p = dynamic_host();
    s.execute(&format!("GET DESCRIPTOR 'D' VALUE {n} N = {field}"), &mut p).unwrap();
    p.value("N").to_string()
}

/// The text field `field` of item `n` of descriptor area 'D'.
fn text_field(s: &Session, n: i64, field: &str) -> String {
    let mut p = dynamic_host();
    s.execute(&format!("GET DESCRIPTOR 'D' VALUE {n} TXT = {field}"), &mut p).unwrap();
    p.value("TXT").to_string().trim_end().to_owned()
}

/// COUNT of descriptor area 'D'.
fn count(s: &Session) -> String {
    let mut p = dynamic_host();
    s.execute("GET DESCRIPTOR 'D' N = COUNT", &mut p).unwrap();
    p.value("N").to_string()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.2 SR1, GR1-GR3, §17.3 GR1,
/// §5.4 SR16, SR17, §4.22.6] ALLOCATE DESCRIPTOR, in direct SQL too, of a
/// name that is an `<identifier>`, LOCAL and GLOBAL apart, of 1 to 1000
/// occurrences; DEALLOCATE DESCRIPTOR of one allocated; each transaction
/// initiating; a session holds 64 at most, more valid SQL beyond a limit
/// (SQLCODE -900).
#[test]
fn allocate_and_deallocate_descriptor_17_2_17_3() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    assert!(s.direct("ALLOCATE DESCRIPTOR 'D1' WITH MAX 5").is_ok());
    assert!(s.transaction_mode().is_some(), "§4.22.6");
    assert_eq!(run(&s, "ALLOCATE DESCRIPTOR LOCAL 'D1'", &mut p), Err("33000"), "GR2: LOCAL is implicit");
    assert_eq!(run(&s, "ALLOCATE DESCRIPTOR GLOBAL ' D1'", &mut p), Ok(Completion::Success), "GLOBAL apart");
    assert_eq!(run(&s, "ALLOCATE DESCRIPTOR 'D2' WITH MAX 0", &mut p), Err("07009"), "GR3");
    assert_eq!(run(&s, "ALLOCATE DESCRIPTOR 'D2' WITH MAX 1001", &mut p), Err("07009"), "GR3");
    assert_eq!(code(&s, "ALLOCATE DESCRIPTOR 'D2' WITH MAX 1.5", &mut p), Code::SyntaxRule, "SR1");
    assert_eq!(run(&s, "ALLOCATE DESCRIPTOR 'TWO WORDS'", &mut p), Err("33000"), "GR1");
    assert_eq!(code(&s, "ALLOCATE DESCRIPTOR 5", &mut p), Code::SyntaxRule, "§5.4 SR16");
    assert_eq!(run(&s, "DEALLOCATE DESCRIPTOR 'D9'", &mut p), Err("33000"), "§17.3 GR1");
    assert_eq!(run(&s, "DEALLOCATE DESCRIPTOR 'D1'", &mut p), Ok(Completion::Success));
    assert_eq!(run(&s, "DEALLOCATE DESCRIPTOR 'D1'", &mut p), Err("33000"));
    for n in 0..63_i32 {
        run(&s, &format!("ALLOCATE DESCRIPTOR 'M{n}' WITH MAX 1"), &mut p).unwrap();
    }
    assert_eq!(code(&s, "ALLOCATE DESCRIPTOR 'MORE'", &mut p), Code::Unsupported, "RULES.md D18");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.4 GR3-GR5, GR7, §17.5 GR2,
/// GR3, GR5, §17.1 SR2] SET DESCRIPTOR sets COUNT and an item's fields,
/// DATA of a value whose type matches the item's; GET DESCRIPTOR gets
/// them into targets; an item beyond COUNT is no data; an item number not
/// of the occurrences is refused (07009); DATA of another type (22005);
/// DATA got of a negative INDICATOR, without it (22002).
#[test]
fn set_and_get_descriptor_17_4_17_5() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    run(&s, "ALLOCATE DESCRIPTOR 'D' WITH MAX 2", &mut p).unwrap();
    assert_eq!(count(&s), "0");
    run(&s, "SET DESCRIPTOR 'D' COUNT = 1", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 TYPE = 4, DATA = K", &mut p).unwrap();
    assert_eq!(run(&s, "GET DESCRIPTOR 'D' VALUE 1 N = TYPE, VAL = DATA", &mut p), Ok(Completion::Success));
    assert_eq!((p.value("N").to_string(), p.value("VAL").to_string()), ("4".to_owned(), "1".to_owned()));
    p.set("SN", Value::Char("hello".to_owned()));
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 TYPE = 12, LENGTH = 128, DATA = SN", &mut p).unwrap();
    run(&s, "GET DESCRIPTOR 'D' VALUE 1 TEXT = DATA", &mut p).unwrap();
    assert_eq!(p.value("TEXT").to_string(), "hello", "a target of no shorter length matches");
    assert_eq!(run(&s, "SET DESCRIPTOR 'D' VALUE 1 DATA = 'hello'", &mut p), Err("22005"), "§17.5 GR5: CHARACTER(5)");
    assert_eq!(run(&s, "GET DESCRIPTOR 'D' VALUE 1 NM = DATA", &mut p), Err("22005"), "§17.4 GR2");
    assert_eq!(run(&s, "GET DESCRIPTOR 'D' VALUE 2 N = TYPE", &mut p), Ok(Completion::NoData), "GR4");
    assert_eq!(run(&s, "GET DESCRIPTOR 'D' VALUE 3 N = TYPE", &mut p), Err("07009"), "GR3");
    assert_eq!(run(&s, "SET DESCRIPTOR 'D' VALUE 0 TYPE = 1", &mut p), Err("07009"), "§17.5 GR2");
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 INDICATOR = -1", &mut p).unwrap();
    assert_eq!(run(&s, "GET DESCRIPTOR 'D' VALUE 1 TEXT = DATA", &mut p), Err("22002"), "GR5");
    assert_eq!(run(&s, "GET DESCRIPTOR 'D' VALUE 1 TEXT = DATA, N = INDICATOR", &mut p), Ok(Completion::Success));
    assert_eq!((p.value("TEXT").to_string(), p.value("N").to_string()), ("hello".to_owned(), "-1".to_owned()), "DATA left as it was");
    assert_eq!(run(&s, "GET DESCRIPTOR 'X' N = COUNT", &mut p), Err("33000"), "§17.4 GR1");
    assert_eq!(run(&s, "SET DESCRIPTOR 'X' COUNT = 1", &mut p), Err("33000"), "§17.5 GR1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.4 SR1-SR3, §17.5 SR1-SR3,
/// Table 17] the types of GET's targets and SET's values: COUNT, an item
/// number and a numeric field exact with scale 0, a field of a name a
/// character string, of 128 characters at least as a target; SET sets no
/// NAME, and a field once (SQLCODE -103).
#[test]
fn the_types_of_fields_17_4_17_5() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    run(&s, "ALLOCATE DESCRIPTOR 'D'", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'D' COUNT = 1", &mut p).unwrap();
    for broken in [
        "GET DESCRIPTOR 'D' TXT = COUNT",
        "GET DESCRIPTOR 'D' VALUE 1 TXT = TYPE",
        "GET DESCRIPTOR 'D' VALUE 1 NM = NAME",
        "GET DESCRIPTOR 'D' VALUE 1 N = NAME",
        "GET DESCRIPTOR 'D' VALUE 'one' N = TYPE",
        "SET DESCRIPTOR 'D' COUNT = 'one'",
        "SET DESCRIPTOR 'D' VALUE 1 TYPE = 'one'",
        "SET DESCRIPTOR 'D' VALUE 1 CHARACTER_SET_NAME = 5",
        "SET DESCRIPTOR 'D' VALUE 1 NAME = 'X'",
        "SET DESCRIPTOR 'D' VALUE 1 TYPE = 1, TYPE = 2",
        "SET DESCRIPTOR 'D' VALUE 1 NULLABLE = 1",
    ] {
        assert_eq!(code(&s, broken, &mut p), Code::SyntaxRule, "{broken}");
    }
    assert_eq!(run(&s, "GET DESCRIPTOR 'D' VALUE 1 TXT = NAME", &mut p), Ok(Completion::Success));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.5 GR4, GR5a-j] TYPE set
/// sets the fields its type takes: a character string's character set,
/// SQL_TEXT, and LENGTH 1; a bit string's LENGTH 1; a datetime's PRECISION
/// 0; an interval's DATETIME_INTERVAL_PRECISION 2; NUMERIC's and
/// DECIMAL's SCALE 0 and PRECISION 18, the default; FLOAT's PRECISION 53;
/// DATETIME_INTERVAL_CODE set, PRECISION 6 of a TIMESTAMP and of an
/// interval to SECOND, else 0.
#[test]
fn type_set_sets_its_fields_17_5() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    run(&s, "ALLOCATE DESCRIPTOR 'D'", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'D' COUNT = 1", &mut p).unwrap();
    let set = |sql: &str, p: &mut Params| run(&s, &format!("SET DESCRIPTOR 'D' VALUE 1 {sql}"), p).unwrap();
    set("TYPE = 1, LENGTH = 9", &mut p);
    assert_eq!((field(&s, 1, "LENGTH"), text_field(&s, 1, "CHARACTER_SET_SCHEMA")), ("9".to_owned(), "INFORMATION_SCHEMA".to_owned()));
    set("TYPE = 12", &mut p);
    assert_eq!((field(&s, 1, "LENGTH"), text_field(&s, 1, "CHARACTER_SET_NAME")), ("1".to_owned(), "SQL_TEXT".to_owned()));
    set("TYPE = 14", &mut p);
    assert_eq!(field(&s, 1, "LENGTH"), "1");
    set("TYPE = 2", &mut p);
    assert_eq!((field(&s, 1, "PRECISION"), field(&s, 1, "SCALE")), ("18".to_owned(), "0".to_owned()));
    set("TYPE = 6", &mut p);
    assert_eq!(field(&s, 1, "PRECISION"), "53");
    set("TYPE = 9, PRECISION = 3", &mut p);
    assert_eq!(field(&s, 1, "PRECISION"), "3", "GR4: PRECISION after TYPE");
    set("DATETIME_INTERVAL_CODE = 3", &mut p);
    assert_eq!(field(&s, 1, "PRECISION"), "6");
    set("DATETIME_INTERVAL_CODE = 1", &mut p);
    assert_eq!(field(&s, 1, "PRECISION"), "0");
    set("TYPE = 10", &mut p);
    assert_eq!(field(&s, 1, "DATETIME_INTERVAL_PRECISION"), "2");
    set("DATETIME_INTERVAL_CODE = 10", &mut p);
    assert_eq!(field(&s, 1, "PRECISION"), "6");
    set("DATETIME_INTERVAL_CODE = 7", &mut p);
    assert_eq!((field(&s, 1, "PRECISION"), field(&s, 1, "DATETIME_INTERVAL_PRECISION")), ("0".to_owned(), "2".to_owned()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.8 GR2, §17.9 GR3, GR4,
/// §17.1 Tables 18-20, §6.1 SR27] DESCRIBE OUTPUT describes each column:
/// TYPE its code, the fields its type takes, NULLABLE, NAME and UNNAMED,
/// a datetime's length in positions; DESCRIBE INPUT each dynamic
/// parameter, NULLABLE 1 and UNNAMED 1; a statement of no row has COUNT
/// 0; more columns than occurrences warn (01005), COUNT their number.
#[test]
fn describe_by_the_codes_of_tables_18_to_20_17_9() {
    let db = setup();
    let s = db.session("APP");
    s.run(
        "CREATE TABLE W (A CHAR(4) NOT NULL, B VARCHAR(7), C NUMERIC(5,2), D DECIMAL(4), E INTEGER, F SMALLINT, G FLOAT(10), \
         H REAL, I DOUBLE PRECISION, J BIT(3), L BIT VARYING(9), M DATE, O TIME(3), Q TIMESTAMP(2) WITH TIME ZONE, \
         R INTERVAL DAY(3) TO SECOND(4), S INTERVAL YEAR TO MONTH, X TIME WITH TIME ZONE)",
    )
    .unwrap();
    let mut p = dynamic_host();
    text(&mut p, "SELECT W.*, E + 1 FROM W");
    run(&s, "PREPARE S FROM TEXT", &mut p).unwrap();
    run(&s, "ALLOCATE DESCRIPTOR 'D' WITH MAX 20", &mut p).unwrap();
    assert_eq!(run(&s, "DESCRIBE S USING SQL DESCRIPTOR 'D'", &mut p), Ok(Completion::Success));
    assert_eq!(count(&s), "18");
    let types: Vec<String> = (1..=18).map(|n| field(&s, n, "TYPE")).collect();
    assert_eq!(types, ["1", "12", "2", "3", "4", "5", "6", "7", "8", "14", "15", "9", "9", "9", "10", "10", "9", "2"]);
    let lengths: Vec<String> = [1, 2, 10, 11, 12, 13, 14, 17].into_iter().map(|n| field(&s, n, "LENGTH")).collect();
    assert_eq!(lengths, ["4", "7", "3", "9", "10", "12", "28", "14"], "§6.1 SR27 of a datetime");
    let codes: Vec<String> = [12, 13, 14, 15, 16, 17].into_iter().map(|n| field(&s, n, "DATETIME_INTERVAL_CODE")).collect();
    assert_eq!(codes, ["1", "2", "5", "10", "7", "4"], "Tables 19 and 20");
    let precisions: Vec<String> = [3, 4, 7, 13, 14, 15].into_iter().map(|n| field(&s, n, "PRECISION")).collect();
    assert_eq!(precisions, ["5", "4", "24", "3", "2", "4"]);
    assert_eq!((field(&s, 3, "SCALE"), field(&s, 15, "DATETIME_INTERVAL_PRECISION")), ("2".to_owned(), "3".to_owned()));
    assert_eq!((field(&s, 1, "OCTET_LENGTH"), field(&s, 11, "OCTET_LENGTH")), ("16".to_owned(), "2".to_owned()));
    assert_eq!((field(&s, 1, "NULLABLE"), field(&s, 2, "NULLABLE")), ("0".to_owned(), "1".to_owned()));
    assert_eq!((text_field(&s, 2, "NAME"), field(&s, 2, "UNNAMED")), ("B".to_owned(), "0".to_owned()));
    assert_eq!((text_field(&s, 18, "NAME"), field(&s, 18, "UNNAMED")), (String::new(), "1".to_owned()));
    assert_eq!(text_field(&s, 1, "COLLATION_NAME"), "SQL_TEXT");
    text(&mut p, "INSERT INTO W (B, E) VALUES (?, ?)");
    run(&s, "PREPARE I FROM TEXT", &mut p).unwrap();
    run(&s, "DESCRIBE INPUT I USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    assert_eq!((count(&s), field(&s, 1, "TYPE"), field(&s, 2, "TYPE")), ("2".to_owned(), "12".to_owned(), "4".to_owned()));
    assert_eq!((field(&s, 1, "NULLABLE"), field(&s, 1, "UNNAMED")), ("1".to_owned(), "1".to_owned()));
    run(&s, "DESCRIBE OUTPUT I USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    assert_eq!(count(&s), "0", "GR3b: of no row");
    run(&s, "ALLOCATE DESCRIPTOR 'SMALL' WITH MAX 2", &mut p).unwrap();
    let warned = Completion::Warning(SqlState::INSUFFICIENT_ITEM_DESCRIPTOR_AREAS);
    assert_eq!(run(&s, "DESCRIBE S USING SQL DESCRIPTOR 'SMALL'", &mut p), Ok(warned), "GR3d");
    s.execute("GET DESCRIPTOR 'SMALL' N = COUNT", &mut p).unwrap();
    assert_eq!(p.value("N").to_string(), "18");
    assert_eq!(run(&s, "DESCRIBE S USING SQL DESCRIPTOR 'NONE'", &mut p), Err("33000"), "GR2");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.9 GR5b-g, §17.10 GR6]
/// EXECUTE USING SQL DESCRIPTOR: each dynamic parameter the DATA of its
/// item, the null value of a negative INDICATOR; COUNT not of the
/// occurrences is refused (07008), not the parameters' number (07001), an
/// item not valid or DATA not of its type (07001).
#[test]
fn execute_using_a_descriptor_17_9() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    text(&mut p, "INSERT INTO T (ID, NAME) VALUES (?, ?)");
    run(&s, "PREPARE S FROM TEXT", &mut p).unwrap();
    run(&s, "ALLOCATE DESCRIPTOR 'D' WITH MAX 2", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE S USING SQL DESCRIPTOR 'D'", &mut p), Err("07001"), "COUNT 0");
    run(&s, "SET DESCRIPTOR 'D' COUNT = 2", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE S USING SQL DESCRIPTOR 'D'", &mut p), Err("07001"), "GR5b iii: no TYPE");
    run(&s, "DESCRIBE INPUT S USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE S USING SQL DESCRIPTOR 'D'", &mut p), Err("07001"), "GR5b iv: no DATA");
    p.set("K", number(9));
    p.set("NM", Value::Char("ZED".to_owned()));
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 DATA = K", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'D' VALUE 2 DATA = NM", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE S USING SQL DESCRIPTOR 'D'", &mut p), Ok(Completion::Success));
    p.set("K", number(10));
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 INDICATOR = 0, DATA = K", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'D' VALUE 2 INDICATOR = -1", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE S USING SQL DESCRIPTOR 'D'", &mut p), Ok(Completion::Success), "a null NAME");
    assert_eq!(rows(&s, "SELECT ID, NAME FROM T WHERE ID > 8 ORDER BY ID").unwrap(), ["9,ZED   ", "10,NULL"]);
    run(&s, "SET DESCRIPTOR 'D' COUNT = 3", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE S USING SQL DESCRIPTOR 'D'", &mut p), Err("07008"), "GR5b i");
    run(&s, "SET DESCRIPTOR 'D' COUNT = -1", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE S USING SQL DESCRIPTOR 'D'", &mut p), Err("07008"), "GR5b i");
    run(&s, "SET DESCRIPTOR 'D' COUNT = 2", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 TYPE = 9, DATETIME_INTERVAL_CODE = 1, DATA = DATE '2026-10-30'", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE S USING SQL DESCRIPTOR 'D'", &mut p), Err("07006"), "GR5e: no DATE is an INTEGER");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.9 GR6, GR7, §17.10 GR7]
/// EXECUTE INTO SQL DESCRIPTOR: each column's value cast to its item's
/// type, INDICATOR 0 and DATA the value, or INDICATOR -1 of the null
/// value; RETURNED_LENGTH and RETURNED_OCTET_LENGTH of a varying string;
/// COUNT not the columns' number refused (07002), and an item's type no
/// column's value is cast to (07006).
#[test]
fn execute_into_a_descriptor_17_9() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    text(&mut p, "SELECT ID, CAST(NAME AS VARCHAR(10)), AMT FROM T WHERE ID = ?");
    run(&s, "PREPARE Q FROM TEXT", &mut p).unwrap();
    run(&s, "ALLOCATE DESCRIPTOR 'D'", &mut p).unwrap();
    run(&s, "DESCRIBE OUTPUT Q USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'D' VALUE 2 TYPE = 12, LENGTH = 128", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE Q INTO SQL DESCRIPTOR 'D' USING K", &mut p), Ok(Completion::Success));
    run(&s, "GET DESCRIPTOR 'D' VALUE 1 VAL = DATA", &mut p).unwrap();
    run(&s, "GET DESCRIPTOR 'D' VALUE 2 SN = DATA, N = RETURNED_LENGTH", &mut p).unwrap();
    assert_eq!(
        (p.value("VAL").to_string(), p.value("SN").to_string(), p.value("N").to_string()),
        ("1".into(), "ANN   ".into(), "6".into())
    );
    p.set("K", number(3));
    run(&s, "EXECUTE Q INTO SQL DESCRIPTOR 'D' USING K", &mut p).unwrap();
    assert_eq!((field(&s, 2, "INDICATOR"), field(&s, 3, "INDICATOR")), ("-1".to_owned(), "-1".to_owned()));
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 TYPE = 9, DATETIME_INTERVAL_CODE = 1", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE Q INTO SQL DESCRIPTOR 'D' USING K", &mut p), Err("07006"), "GR7c");
    run(&s, "SET DESCRIPTOR 'D' COUNT = 2", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE Q INTO SQL DESCRIPTOR 'D' USING K", &mut p), Err("07002"), "GR6b ii");
    run(&s, "SET DESCRIPTOR 'D' COUNT = 3", &mut p).unwrap();
    run(&s, "SET DESCRIPTOR 'D' VALUE 1 TYPE = 0", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE Q INTO SQL DESCRIPTOR 'D' USING K", &mut p), Err("07002"), "GR6b iii");
}
