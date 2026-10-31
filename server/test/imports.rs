//! Import over a connection (RULES.md D59): a source's records inserted
//! into a table through a prepared INSERT, a batch to a transaction; a
//! failure rolls its batch back and says the record, or a bad row is
//! skipped and reported; any program's source as well as the built-in
//! ones.

use std::io::Read;
use std::net::TcpStream;

use turborel::db::Database;
use turborel::value::{DataType, Decimal, Value};
use turborel_client::connection::Connection;
use turborel_client::import::{
    Cause, Delimited, Dialect, Heading, Import, ImportError, Imported, Position, Record, Rejected, Source, SourceError, import,
};

use crate::common::{Running, rows, start, state};

/// A database whose schema X holds STAFF, of an ID that is its primary
/// key, a NAME, a date HIRED and PAY.
fn staff() -> Database {
    let db = Database::new();
    let table = "CREATE TABLE STAFF (ID INTEGER NOT NULL PRIMARY KEY, NAME VARCHAR(20), HIRED DATE, PAY DECIMAL(8,2) DEFAULT 1)";
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION X {table}")).unwrap();
    db
}

/// `text` imported as CSV, written as `dialect` says, over `c`, as `how`
/// says: what it did, or why it stopped; and the rows skipped.
fn csv(c: &mut Connection<TcpStream>, text: &str, dialect: Dialect, how: &Import) -> (Result<Imported, ImportError>, Vec<Rejected>) {
    let mut source = Delimited::new(text.as_bytes(), dialect);
    let mut skipped = Vec::new();
    let done = import(c, &mut source, how, &mut |r| skipped.push(r));
    (done, skipped)
}

/// The rows of STAFF, as `c` sees them, in order of ID; the transaction
/// that read them ended.
fn table(c: &mut Connection<TcpStream>) -> Vec<String> {
    let all = rows(&c.query("SELECT ID, NAME, HIRED, PAY FROM STAFF ORDER BY ID").unwrap());
    c.query("COMMIT").unwrap();
    all
}

/// An import into STAFF, `batch` rows at a time, skipping bad rows if
/// `skip`.
fn into_staff(batch: u64, skip: bool) -> Import {
    Import { batch, skip, ..Import::into("STAFF") }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6 GR5, §17.9 GR3, GR5f,
/// g]; [DESIGN] RULES.md D59: a source's records go into the table, each
/// field into the column its heading names, read as an identifier, or
/// without headings into the table's columns in order, as a prepared
/// query describes them; each value cast to its column's type, a null to
/// null, a column not named its default; the table named in any way a
/// `<table name>` may be; committed a batch at a time, and nothing left
/// active.
#[test]
fn an_import_inserts_a_sources_records() {
    let db = staff();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let headed = "name,\"ID\",hired\nAda,1,1980-01-02\n\"Hopper, Grace\",2,\n";
    let (done, skipped) = csv(&mut c, headed, Dialect::csv().with_header(), &into_staff(1, false));
    assert_eq!(done.unwrap(), Imported { rows: 2, rejected: 0 });
    assert_eq!(skipped, Vec::<Rejected>::new());
    assert!(!c.in_transaction(), "every batch committed");
    let by_position = "3\tEve\t\\N\t12.5\n4\t\t1999-12-31\t\\N\n";
    let tsv = Dialect::tsv().with_null("\\N");
    let (done, _) = csv(&mut c, by_position, tsv, &Import::into("x.staff"));
    assert_eq!(done.unwrap().rows, 2);
    let (done, _) = csv(&mut c, "5,Bob,,2\n", Dialect::csv(), &Import::into("\"X\" . \"STAFF\""));
    assert_eq!(done.unwrap().rows, 1);
    assert_eq!(
        table(&mut c),
        ["1,Ada,1980-01-02,1.00", "2,Hopper, Grace,NULL,1.00", "3,Eve,NULL,12.50", "4,,1999-12-31,NULL", "5,Bob,NULL,2.00",]
    );
    let (done, _) = csv(&mut c, "", Dialect::csv(), &Import::into("STAFF"));
    assert_eq!(done.unwrap(), Imported::default(), "nothing to import");
    c.terminate().unwrap();
}

/// Verifies: [DESIGN] RULES.md D59: a row the server refuses stops the
/// import, saying its record and line, the server's failure, the rows
/// committed and the batch rolled back; so does a record of too many or
/// too few fields, and one the source cannot read; the batches before it
/// are kept.
#[test]
fn an_import_stops_at_a_bad_row_rolling_back_its_batch() {
    let db = staff();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let text = "ID,NAME\n1,a\n2,b\n3,c\nfour,d\n5,e\n";
    let (done, _) = csv(&mut c, text, Dialect::csv().with_header(), &into_staff(2, false));
    let stopped = done.unwrap_err();
    assert_eq!(stopped.position, Some(Position { record: 4, line: Some(5) }));
    let Cause::Sql(failure) = &stopped.cause else { panic!("{stopped:?}") };
    assert_eq!(state(failure), ("22018", -301), "{failure}");
    assert_eq!((stopped.committed, stopped.rolled_back), (2, 1));
    let told = stopped.to_string();
    assert!(told.starts_with("record 4, line 5: SQLCODE -301 (SQLSTATE 22018): "), "{told}");
    assert!(told.ends_with("; 2 rows committed, 1 rolled back"), "{told}");
    assert!(!c.in_transaction(), "the batch rolled back");
    assert_eq!(table(&mut c), ["1,a,NULL,1.00", "2,b,NULL,1.00"]);
    let (done, _) = csv(&mut c, "6,f,,1\n7,g\n", Dialect::csv(), &into_staff(0, false));
    let short = done.unwrap_err();
    assert_eq!(short.to_string(), "record 2, line 2: [DESIGN] 2 fields, for 4 columns; 0 rows committed, 1 rolled back");
    let (done, _) = csv(&mut c, "ID\n8\n\"9\n", Dialect::csv().with_header(), &into_staff(0, false));
    let unread = done.unwrap_err();
    assert_eq!(unread.to_string(), "record 2, line 3: [RFC 4180 §2 rule 5] a quoted field is not closed; 0 rows committed, 1 rolled back");
    let (done, _) = csv(&mut c, "ID\n1\n", Dialect::csv().with_header(), &into_staff(0, false));
    let duplicate = done.unwrap_err();
    let Cause::Sql(failure) = &duplicate.cause else { panic!("{duplicate:?}") };
    assert_eq!(failure.sqlstate, "23000", "{failure}");
    assert_eq!(table(&mut c).len(), 2, "nothing more kept");
    c.terminate().unwrap();
}

/// Verifies: [DESIGN] RULES.md D59: told to skip bad rows, an import
/// reports each, its record, line and why, and inserts the rest.
#[test]
fn an_import_skips_bad_rows_when_asked() {
    let db = staff();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let text = "ID,NAME\n1,a\nx,b\n3\n1,again\n5,e\n";
    let (done, skipped) = csv(&mut c, text, Dialect::csv().with_header(), &into_staff(2, true));
    assert_eq!(done.unwrap(), Imported { rows: 2, rejected: 3 });
    let told: Vec<String> = skipped.iter().map(ToString::to_string).collect();
    assert!(told[0].starts_with("record 2, line 3: SQLCODE -301 (SQLSTATE 22018): "), "{told:?}");
    assert!(told[0].ends_with("; skipped"), "{told:?}");
    assert_eq!(told[1], "record 3, line 4: [DESIGN] 1 fields, for 2 columns; skipped");
    assert_eq!(skipped[2].position, Position { record: 4, line: Some(5) });
    assert_eq!(table(&mut c), ["1,a,NULL,1.00", "5,e,NULL,1.00"]);
    // Every row skipped: what they began is ended all the same.
    let (done, skipped) = csv(&mut c, "ID\n1\n", Dialect::csv().with_header(), &into_staff(0, true));
    assert_eq!((done.unwrap(), skipped.len()), (Imported { rows: 0, rejected: 1 }, 1));
    assert!(!c.in_transaction());
    c.terminate().unwrap();
}

/// Verifies: [DESIGN] RULES.md D59: an import is refused before any row
/// in a transaction active, which it would commit; of a table not named
/// as a `<table name>`, nor one that exists; of headings naming no column
/// of it; and of a source whose headings cannot be read.
#[test]
fn an_import_is_refused_before_any_row() {
    let db = staff();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    c.query("INSERT INTO STAFF (ID) VALUES (1)").unwrap();
    let (done, _) = csv(&mut c, "2\n", Dialect::csv(), &Import::into("STAFF"));
    let active = done.unwrap_err();
    assert_eq!(
        active.to_string(),
        "[DESIGN] an import begins with no transaction active, as it commits its batches; 0 rows committed, 0 rolled back"
    );
    assert!(c.in_transaction(), "left as it was");
    c.query("ROLLBACK").unwrap();
    for wrong in ["STAFF; DROP TABLE STAFF", "1STAFF", "A.B.C.D", "X.", "'STAFF'", ""] {
        let (done, _) = csv(&mut c, "2\n", Dialect::csv(), &Import::into(wrong));
        let refused = done.unwrap_err();
        assert_eq!(refused.cause, Cause::Input(format!("[ISO/IEC 9075:1992 §5.4] {wrong} is not a <table name>")), "{wrong}");
    }
    let (done, _) = csv(&mut c, "2\n", Dialect::csv(), &Import::into("NOPE"));
    let Cause::Sql(missing) = done.unwrap_err().cause else { panic!("no failure") };
    assert_eq!(state(&missing), ("37000", -103), "{missing}");
    let (done, _) = csv(&mut c, "ID,WAGE\n2,3\n", Dialect::csv().with_header(), &Import::into("STAFF"));
    let Cause::Sql(unknown) = done.unwrap_err().cause else { panic!("no failure") };
    assert!(unknown.message.contains("WAGE"), "{unknown}");
    let (done, _) = csv(&mut c, "", Dialect::csv().with_header(), &Import::into("STAFF"));
    let headless = done.unwrap_err();
    assert_eq!(headless.position, Some(Position { record: 0, line: Some(1) }));
    assert_eq!(table(&mut c), Vec::<String>::new());
    c.terminate().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §14.3 GR5a]; [DESIGN] RULES.md
/// D59: a commit of a batch that fails, a deferred constraint not
/// satisfied, stops the import, the batch rolled back.
#[test]
fn a_batch_whose_commit_fails_is_rolled_back() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE U (K INTEGER UNIQUE DEFERRABLE INITIALLY DEFERRED)").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let mut source = Delimited::new(&b"1\n2\n3\n3\n"[..], Dialect::csv());
    let stopped = import(&mut c, &mut source, &Import { batch: 2, ..Import::into("U") }, &mut |_| {}).unwrap_err();
    assert_eq!(stopped.position, None);
    let Cause::Sql(failure) = &stopped.cause else { panic!("{stopped:?}") };
    assert_eq!(failure.sqlstate, "40002", "{failure}");
    assert_eq!((stopped.committed, stopped.rolled_back), (2, 2));
    assert_eq!(rows(&c.query("SELECT K FROM U ORDER BY K").unwrap()), ["1", "2"]);
    c.terminate().unwrap();
}

/// A program's own source: `count` records of an ID and a NAME, made as
/// they are read, typed, with headings that say their types; the
/// connection `ended` by the operator of `server` as record `end_at` is
/// read, if one is given.
struct Made<'a> {
    count: u64,
    next: u64,
    end_at: Option<(u64, &'a Running)>,
}

impl Source for Made<'_> {
    fn headings(&mut self) -> Result<Option<Vec<Heading>>, SourceError> {
        let id = Heading { name: "id".into(), data_type: Some(DataType::Varying { length: 10 }) };
        Ok(Some(vec![id, Heading { name: "NAME".into(), data_type: None }]))
    }

    fn record(&mut self) -> Result<Option<Record>, SourceError> {
        if self.next == self.count {
            return Ok(None);
        }
        self.next += 1;
        if let Some((at, server)) = self.end_at
            && at == self.next
        {
            let (id, _) = server.operator.connections().into_iter().find(|(_, u)| u.as_deref() == Some("X")).unwrap();
            assert!(server.operator.end(id));
        }
        let id = Value::Exact(Decimal::new(i128::from(self.next), 0));
        Ok(Some(Record { values: vec![id, Value::Char(format!("n{}", self.next))], position: Position { record: self.next, line: None } }))
    }
}

/// Verifies: [DESIGN] RULES.md D59: a program's source of typed values,
/// made as they are read, imports as the built-in ones do, many batches
/// of it, none held but the record under way; a connection lost stops
/// the import, even one that skips bad rows, saying the record, which
/// has no line.
#[test]
fn a_programs_own_source_imports() {
    let db = staff();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let mut made = Made { count: 2_500, next: 0, end_at: None };
    assert_eq!(import(&mut c, &mut made, &into_staff(1_000, false), &mut |_| {}).unwrap().rows, 2_500);
    assert_eq!(rows(&c.query("SELECT COUNT(*), MAX(NAME) FROM STAFF").unwrap()), ["2500,n999"]);
    c.query("DELETE FROM STAFF").unwrap();
    c.query("COMMIT").unwrap();
    let mut ended = Made { count: 10, next: 0, end_at: Some((3, &server)) };
    let lost = import(&mut c, &mut ended, &into_staff(0, true), &mut |_| {}).unwrap_err();
    let Cause::Sql(failure) = &lost.cause else { panic!("{lost:?}") };
    assert_eq!(state(failure), ("08006", -919));
    assert_eq!(lost.position, Some(Position { record: 3, line: None }));
    assert_eq!(lost.to_string().split(": ").next(), Some("record 3"));
    assert_eq!((lost.committed, lost.rolled_back), (0, 2));
    assert!(c.is_lost());
    let mut check = server.connect("X");
    assert_eq!(rows(&check.query("SELECT COUNT(*) FROM STAFF").unwrap()), ["0"], "the batch rolled back with the session");
    check.terminate().unwrap();
}

/// CSV of `rows` records, made as it is read, never held whole: each an
/// ID and a NAME of 20 characters.
struct Generated {
    rows: u64,
    made: u64,
    line: Vec<u8>,
}

impl Read for Generated {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.line.is_empty() {
            if self.made == self.rows {
                return Ok(0);
            }
            self.made += 1;
            self.line = format!("{},\"{}\"\n", self.made, "x".repeat(20)).into_bytes();
        }
        let n = buf.len().min(self.line.len());
        buf[..n].copy_from_slice(&self.line[..n]);
        self.line.drain(..n);
        Ok(n)
    }
}

/// Verifies: [DESIGN] RULES.md D59: a CSV source larger than any batch
/// streams from its reader, a record at a time, so its size bounds
/// nothing held: here, many batches of it, its rows all committed.
#[test]
fn a_large_source_streams() {
    let db = staff();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let reader = std::io::BufReader::with_capacity(4096, Generated { rows: 20_000, made: 0, line: b"ID,NAME\n".to_vec() });
    let mut source = Delimited::new(reader, Dialect::csv().with_header().with_max_record(256));
    let imported = import(&mut c, &mut source, &Import { table: "STAFF".into(), batch: 5_000, skip: false }, &mut |_| {}).unwrap();
    assert_eq!(imported.rows, 20_000);
    assert_eq!(rows(&c.query("SELECT COUNT(*), MIN(ID), MAX(ID) FROM STAFF").unwrap()), ["20000,1,20000"]);
    c.terminate().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.8 GR4b iii, §9.2 GR3b,
/// GR3e]; [DESIGN] RULES.md D59: a string for a character string column
/// is assigned to it as INSERT assigns: one too long is refused (SQLSTATE
/// 22001), not cut as the CAST of a parameter's value would cut it, with
/// a warning [§6.10 GR5c ii, GR6c ii]; spaces beyond the length are cut.
/// Regression test (R-TEST-7): a name too long was cut, and imported.
#[test]
fn a_string_too_long_for_its_column_is_refused() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER, F CHARACTER(3), V VARCHAR(3))").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let text = "1,ab,abc   \n2,abcd,a\n3,a,abcd\n";
    let mut source = Delimited::new(text.as_bytes(), Dialect::csv());
    let mut skipped = Vec::new();
    let how = Import { skip: true, ..Import::into("T") };
    assert_eq!(import(&mut c, &mut source, &how, &mut |r| skipped.push(r)).unwrap(), Imported { rows: 1, rejected: 2 });
    for (row, record) in skipped.iter().zip([2_u64, 3_u64]) {
        assert_eq!(row.position.record, record);
        let Cause::Sql(failure) = &row.cause else { panic!("{row:?}") };
        assert_eq!(failure.sqlstate, "22001", "{failure}");
        assert!(failure.rule.as_deref().is_some_and(|r| r.starts_with("ISO/IEC 9075:1992 §13.8 GR4b iii, §9.2 GR3")), "{failure:?}");
    }
    assert_eq!(rows(&c.query("SELECT K, F || '|', V || '|' FROM T").unwrap()), ["1,ab |,abc|"]);
    c.terminate().unwrap();
}
