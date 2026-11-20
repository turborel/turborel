//! Import of what the first import tests do not reach: large objects
//! sent ahead by their locators, headings that are key words or not
//! identifiers at all, and a bad row of a program's source not skipped
//! (RULES.md D59).

use turborel::db::Database;
use turborel::value::{Binary, DataType, Decimal, Value};
use turborel_client::import::{Cause, Heading, Import, Position, Record, Rejected, Source, SourceError, import};

use crate::common::{rows, start, state};

/// A program's source of the records `records`, its fields headed
/// `headings`.
struct Given {
    headings: Vec<Heading>,
    records: std::vec::IntoIter<Vec<Value>>,
    next: u64,
}

impl Given {
    /// A source of `records`, each field headed by the name in `names`.
    fn of(names: &[&str], records: Vec<Vec<Value>>) -> Given {
        let headings = names.iter().map(|n| Heading { name: (*n).to_owned(), data_type: None }).collect();
        Given { headings, records: records.into_iter(), next: 0 }
    }
}

impl Source for Given {
    fn headings(&mut self) -> Result<Option<Vec<Heading>>, SourceError> {
        Ok(Some(self.headings.clone()))
    }

    fn record(&mut self) -> Result<Option<Record>, SourceError> {
        let Some(values) = self.records.next() else { return Ok(None) };
        self.next += 1;
        Ok(Some(Record { values, position: Position { record: self.next, line: None } }))
    }
}

/// An exact number of `n`.
fn exact(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.37.5]; [DESIGN] RULES.md D59: a
/// value for a large object column longer than a message holds whole is
/// sent ahead a piece at a time and given by its locator, character and
/// binary alike, and reads back whole; a short one, and a null, go as
/// they are.
#[test]
fn large_objects_are_imported_by_their_locators() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE DOCS (ID INTEGER, C CLOB(1M), B BLOB(1M))").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let (long, chars) = (70_000_usize, 35_000_usize);
    let records = vec![
        vec![exact(1), Value::Char("é".repeat(chars)), Value::Binary(Binary::new(vec![7; long]))],
        vec![exact(2), Value::Char("short".into()), Value::Null],
    ];
    let mut source = Given::of(&["ID", "C", "B"], records);
    let imported = import(&mut c, &mut source, &Import::into("DOCS"), &mut |_| {}).unwrap();
    assert_eq!(imported.rows, 2);
    let sizes = rows(&c.query("SELECT ID, CHAR_LENGTH(C), OCTET_LENGTH(C), OCTET_LENGTH(B) FROM DOCS ORDER BY ID").unwrap());
    assert_eq!(sizes, [format!("1,{chars},{long},{long}"), "2,5,5,NULL".to_owned()]);
    c.terminate().unwrap();
}

/// Verifies: [ISO/IEC 9075:1992 §5.2 SR10]; [DESIGN] RULES.md D59: a
/// heading that is a key word names the column of its name in upper
/// case, as it would were it not reserved; one that is no identifier at
/// all names the column of its text as written.
#[test]
fn headings_name_columns_as_identifiers_or_as_written() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE ODD (\"VALUE\" INTEGER, \"two words\" INTEGER)").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let mut source = Given::of(&["value", "two words"], vec![vec![exact(1), exact(2)]]);
    assert_eq!(import(&mut c, &mut source, &Import::into("ODD"), &mut |_| {}).unwrap().rows, 1);
    assert_eq!(rows(&c.query("SELECT \"VALUE\", \"two words\" FROM ODD").unwrap()), ["1,2"]);
    c.terminate().unwrap();
}

/// Verifies: [DESIGN] RULES.md D59: a record of a program's source that
/// has too few fields stops an import not told to skip bad rows, saying
/// the record, its batch rolled back; told to skip, it is reported and
/// the rest imported. A value its column cannot be cast to [ISO/IEC
/// 9075:1992 §6.10 SR4] stops the import with the server's failure.
#[test]
fn a_short_record_stops_or_is_skipped() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE P (A INTEGER, B INTEGER)").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let records = || vec![vec![exact(1), exact(1)], vec![exact(2)], vec![exact(3), exact(3)]];
    let stopped = import(&mut c, &mut Given::of(&["A", "B"], records()), &Import::into("P"), &mut |_| {}).unwrap_err();
    assert_eq!(stopped.position, Some(Position { record: 2, line: None }));
    assert!(matches!(stopped.cause, Cause::Input(_)), "{stopped:?}");
    assert_eq!((stopped.committed, stopped.rolled_back), (0, 1));
    let mut skipped: Vec<Rejected> = Vec::new();
    let how = Import { skip: true, ..Import::into("P") };
    let imported = import(&mut c, &mut Given::of(&["A", "B"], records()), &how, &mut |r| skipped.push(r)).unwrap();
    assert_eq!((imported.rows, imported.rejected), (2, 1));
    assert_eq!(skipped.iter().map(|r| r.position.record).collect::<Vec<_>>(), [2]);
    assert_eq!(rows(&c.query("SELECT A FROM P ORDER BY A").unwrap()), ["1", "3"]);
    c.query("COMMIT").unwrap();
    let typed = Heading { name: "A".into(), data_type: Some(DataType::Boolean) };
    let mut wrong = Given { headings: vec![typed], records: vec![vec![Value::Bool(true)]].into_iter(), next: 0 };
    let refused = import(&mut c, &mut wrong, &Import::into("P"), &mut |_| {}).unwrap_err();
    let Cause::Sql(failure) = &refused.cause else { panic!("{refused:?}") };
    assert_eq!(state(failure), ("22005", -301), "{failure}");
    c.terminate().unwrap();
}
