//! Character strings at the lengths where a value stops holding its string
//! itself and shares a buffer instead (RULES.md D960): what is stored, read,
//! compared, grouped, found by an index, joined and cut is what the
//! standard's rules give of the text, whatever holds it. Each expectation
//! is made here of a `String`, as values held their text before.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;
use turborel::value::chars::INLINE;
use turborel::value::{Decimal, Value};

use crate::common::{Scratch, both_ways, failure, rows};

/// The fixed lengths of F's columns: below, at and past what a value
/// holds itself, and past what a padded string is built in on the stack.
const FIXED: [usize; 4] = [8, INLINE, INLINE + 1, 300];

/// The database kept in `dir`, opened.
fn on_files(dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(&crate::common::os(), dir, &OpenOptions::default()).unwrap();
    db
}

/// Schema A, kept in `dir`: F, of an ID, a CHARACTER column of each length
/// of `FIXED` and a CHARACTER VARYING(400).
fn fixed(dir: &Path) -> Database {
    let db = on_files(dir);
    let columns: Vec<String> = FIXED.iter().map(|n| format!("F{n} CHARACTER({n})")).collect();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE F (ID INTEGER NOT NULL, {}, V VARCHAR(400))", columns.join(", ")))
        .unwrap();
    db
}

/// The texts stored: of one octet a character and of several, of lengths
/// around the capacity, with spaces within and at the end.
fn texts() -> Vec<String> {
    let mut texts: Vec<String> = [0, 1, 7, 8, INLINE - 1, INLINE, INLINE + 1, 2 * INLINE, 299, 300]
        .iter()
        .map(|&n| "abcdefg".chars().cycle().take(n).collect())
        .collect();
    texts.extend(["é", "a b ", "ééééééé", "日本語 ", "😀😀"].map(str::to_owned));
    texts.push("é".repeat(INLINE >> 1));
    texts.push(format!("{}é", "x".repeat(INLINE - 1)));
    texts
}

fn int(n: usize) -> Value {
    Value::Exact(Decimal::new(n as i128, 0))
}

/// `text` as a CHARACTER of `n` characters holds it: padded with spaces.
fn padded(text: &str, n: usize) -> String {
    format!("{text}{}", " ".repeat(n - text.chars().count()))
}

/// The strings of the row of F whose ID is `id`.
fn read(s: &Session, id: usize) -> Vec<String> {
    let mut c = s.open_cursor(&format!("SELECT F8, F{INLINE}, F{}, F300, V FROM F WHERE ID = {id}", INLINE + 1)).unwrap();
    let row = c.fetch().unwrap().unwrap();
    row.iter().map(|v| if let Value::Char(s) = v { s.to_string() } else { format!("not a string: {v}") }).collect()
}

/// Verifies: [X3.135-1986 §8.7 GR7b] [ISO/IEC 9075:1992 §9.2 GR3b, GR3d]
/// a string assigned to a CHARACTER column is read back padded with
/// spaces to its length, and one assigned to a CHARACTER VARYING column as
/// it was given, at every length around what a value holds itself, of
/// characters of one octet and of several, given as a parameter and as a
/// literal, after an UPDATE, and once the database is opened again read
/// by batches and from its record row by row; [§9.2 GR3b ii] one longer than the
/// column by more than spaces is refused, SQLSTATE 22001.
#[test]
fn strings_read_back_as_assigned_at_every_length() {
    let scratch = Scratch::new("char-strings");
    let dir = scratch.path("db");
    let db = fixed(&dir);
    let a = db.session("A");
    let mut all = Vec::new();
    let insert = a.prepare("INSERT INTO F VALUES (?, ?, ?, ?, ?, ?)").unwrap();
    for (id, text) in texts().iter().enumerate() {
        let count = text.chars().count();
        let given: Vec<Value> = FIXED.iter().map(|&n| if count <= n { Value::Char(text.as_str().into()) } else { Value::Null }).collect();
        let mut row = vec![int(id)];
        row.extend(given);
        row.push(Value::Char(text.as_str().into()));
        a.execute_prepared(&insert, &row).unwrap();
        a.commit().unwrap();
        let expected = |text: &str| -> Vec<String> {
            let mut all: Vec<String> =
                FIXED.iter().map(|&n| if count <= n { padded(text, n) } else { "not a string: NULL".to_owned() }).collect();
            all.push(text.to_owned());
            all
        };
        assert_eq!(read(&a, id), expected(text), "{text:?} as a parameter");
        // The same text as a literal, each column set again.
        let quoted = format!("'{text}'");
        let sets: Vec<String> = FIXED.iter().filter(|&&n| count <= n).map(|n| format!("F{n} = {quoted}")).collect();
        a.run(&format!("UPDATE F SET {}, V = {quoted} WHERE ID = {id}", sets.join(", ")).replace("SET , ", "SET ")).unwrap();
        a.commit().unwrap();
        assert_eq!(read(&a, id), expected(text), "{text:?} as a literal");
        all.push(expected(text));
        for &n in FIXED.iter().filter(|&&n| count > n) {
            let refused = a.run(&format!("UPDATE F SET F{n} = {quoted} WHERE ID = {id}"));
            let cut = text.chars().skip(n).all(|c| c == ' ');
            assert_eq!(refused.is_ok(), cut, "{text:?} into CHARACTER({n})");
            if !cut {
                assert_eq!(failure(refused), (Code::Data, "22001".to_owned()));
            }
        }
    }
    a.commit().unwrap();
    drop((a, db));
    // Opened again, each row is read by batches and then row by row, from
    // its record.
    let db = on_files(&dir);
    let a = db.session("A");
    for batches in ["ON", "OFF"] {
        a.run(&format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        for (id, expected) in all.iter().enumerate() {
            assert_eq!(&read(&a, id), expected, "row {id} read with EXECUTOR.BATCHES {batches}");
        }
    }
}

/// Schema A: T, of a key K and a string V CHARACTER VARYING(100), indexed.
fn varying() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL, V VARCHAR(100)) CREATE TABLE U (W VARCHAR(100) NOT NULL UNIQUE)",
    )
    .unwrap();
    db
}

/// Verifies: [X3.135-1986 §5.11 GR5] [ISO/IEC 9075:1992 §8.2 GR3a] two
/// strings that differ in trailing spaces alone, one held in its value and
/// one in a buffer, are equal: to a comparison, to DISTINCT and GROUP BY,
/// to a join, to an index's key and to a unique constraint; and strings
/// of the two forms order by their text, read by batches or by rows.
#[test]
fn the_forms_compare_group_and_key_as_their_text() {
    let db = varying();
    let a = db.session("A");
    let short = "x".repeat(INLINE - 2);
    let long = format!("{short}{}", " ".repeat(10));
    let other = format!("{short}y{}", " ".repeat(10));
    for (k, v) in [&short, &long, &other].into_iter().enumerate() {
        a.run(&format!("INSERT INTO T VALUES ({k}, '{v}')")).unwrap();
    }
    a.commit().unwrap();
    let both = (vec!["0".to_owned(), "1".to_owned()], vec!["0".to_owned(), "1".to_owned()]);
    assert_eq!(both_ways(&a, &format!("SELECT K FROM T WHERE V = '{short}'")), both);
    assert_eq!(both_ways(&a, &format!("SELECT K FROM T WHERE V = '{long}'")), both);
    assert_eq!(both_ways(&a, &format!("SELECT K FROM T WHERE V > '{long}'")).0, ["2"]);
    assert_eq!(both_ways(&a, "SELECT COUNT(DISTINCT V), COUNT(*) FROM T").0, ["2,3"]);
    assert_eq!(both_ways(&a, "SELECT COUNT(*) FROM T GROUP BY V").0, ["1", "2"]);
    assert_eq!(both_ways(&a, "SELECT X.K, Y.K FROM T X, T Y WHERE X.V = Y.V AND X.K < Y.K").0, ["0,1"]);
    assert_eq!(rows(&a, "SELECT K FROM T ORDER BY V DESC, K").unwrap(), ["2", "0", "1"]);
    a.commit().unwrap();
    a.run("CREATE INDEX TV ON T (V)").unwrap();
    assert_eq!(both_ways(&a, &format!("SELECT K FROM T WHERE V = '{long}'")), both, "by the index");
    a.run(&format!("INSERT INTO U VALUES ('{short}')")).unwrap();
    assert_eq!(failure(a.run(&format!("INSERT INTO U VALUES ('{long}')"))).1, "23000");
    a.run(&format!("INSERT INTO U VALUES ('{other}')")).unwrap();
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.32 GR2] [ISO/IEC 9075:1992 §6.7]
/// strings concatenated, trimmed, cut, folded and measured give what the
/// same operations give of their text, where the operands and the result
/// are each held in a value or in a buffer.
#[test]
fn string_functions_give_the_same_text_in_either_form() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE ONE (N INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO ONE VALUES (1)").unwrap();
    let value = |sql: &str| rows(&a, &format!("SELECT {sql} FROM ONE")).unwrap_or_else(|e| panic!("{sql}: {e:?}")).remove(0);
    for (left, right) in
        [(0, 0), (1, 0), (0, 1), (INLINE - 1, 1), (INLINE, 0), (INLINE, 1), (16, 16), (16, 17), (INLINE + 1, INLINE + 1), (3, 300)]
    {
        let (l, r) = ("é".repeat(left >> 1) + &"a".repeat(left % 2), "b ".repeat(right >> 1) + &"c".repeat(right % 2));
        let joined = format!("{l}{r}");
        assert_eq!(value(&format!("'{l}' || '{r}'")), joined, "{left} || {right}");
        assert_eq!(value(&format!("CHAR_LENGTH('{l}' || '{r}')")), joined.chars().count().to_string());
        assert_eq!(value(&format!("OCTET_LENGTH('{l}' || '{r}')")), joined.len().to_string());
        assert_eq!(value(&format!("UPPER('{l}' || '{r}')")), joined.to_ascii_uppercase());
        assert_eq!(value(&format!("TRIM(TRAILING FROM '{l}' || '{r}')")), joined.trim_end_matches(' '));
        assert_eq!(value(&format!("'{l}' || '{r}' || '{l}'")), format!("{joined}{l}"));
        let kept: String = joined.chars().skip(1).take(INLINE).collect();
        assert_eq!(value(&format!("SUBSTRING('{l}' || '{r}' FROM 2 FOR {INLINE})")), kept);
        assert_eq!(value(&format!("CAST('{l}' || '{r}' AS CHARACTER(400))")), padded(&joined, 400));
    }
}
