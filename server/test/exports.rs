//! An export's large objects (RULES.md D975, D976): each written as its
//! value, a character one as its text, a binary one in hexits, one sent by
//! its locator read a chunk at a time, however long, and one within a row
//! or an array too; an import reads CSV's back; native rows keep them.

use std::io::{self, Read, Write};
use std::net::TcpStream;

use turborel::db::Database;
use turborel::value::lob::LobKind;
use turborel::value::{Decimal, Value};
use turborel_client::connection::{Connection, Outcome};
use turborel_client::export::{Delimited, ExportFormat, read_native};
use turborel_client::import::{Delimited as Source, Dialect, Import, import};
use turborel_protocol::MAX_BODY;

use crate::common::{done, rows, start, state};

/// The columns of T and U, a CLOB, a BLOB and an NCLOB.
const COLUMNS: &str = "(K INTEGER, C CLOB, B BLOB, N NCLOB)";

/// A database whose schema X holds T and U, of `COLUMNS`.
fn database() -> Database {
    let db = Database::new();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T {COLUMNS} CREATE TABLE U {COLUMNS}")).unwrap();
    db
}

/// A text of `n` characters, some of two octets and some quotes, commas
/// and line breaks, which CSV must quote.
fn text(n: usize) -> String {
    (0..n).map(|i| ['w', 'é', '"', ',', '\n', 'x', ' '][i % 7]).collect()
}

/// `n` octets, of every value.
fn octets(n: usize) -> Vec<u8> {
    (0..n).map(|i| u8::try_from(i % 256).unwrap()).collect()
}

/// A large object of `kind` written over `c` a piece at a time: `bytes`.
fn written(c: &mut Connection<TcpStream>, kind: LobKind, bytes: &[u8]) -> Value {
    let mut writer = c.create_lob(kind).unwrap();
    writer.write_all(bytes).unwrap();
    Value::Lob(writer.finish().unwrap())
}

/// A row of T's values: its key, and a CLOB, BLOB and NCLOB of `c`, `b`
/// and `n`, null if none.
type Given = (i128, Option<String>, Option<Vec<u8>>, Option<String>);

/// The rows the round trip carries: empty values, short ones, ones longer
/// than a reply's body, and nulls.
fn given() -> Vec<Given> {
    let long = usize::try_from(MAX_BODY).unwrap() + 1000;
    vec![
        (1, Some(String::new()), Some(Vec::new()), Some(String::new())),
        (2, Some("a, \"short\" one".to_owned()), Some(vec![0, 1, 0xFE, 0xFF]), Some("ñ".to_owned())),
        (3, Some(text(long)), Some(octets(long)), Some(text(300_000))),
        (4, None, None, None),
    ]
}

/// Inserts `given` into T over `c`, each value of a large object written a
/// piece at a time.
fn insert(c: &mut Connection<TcpStream>, given: &[Given]) {
    let p = c.prepare("INSERT INTO T VALUES (?, ?, ?, ?)").unwrap();
    for (k, clob, blob, nclob) in given {
        let text = |c: &mut Connection<TcpStream>, s: &Option<String>| {
            s.as_ref().map_or(Value::Null, |s| written(c, LobKind::Character, s.as_bytes()))
        };
        let (cv, nv) = (text(c, clob), text(c, nclob));
        let bv = blob.as_ref().map_or(Value::Null, |b| written(c, LobKind::Binary, b));
        assert_eq!(done(&c.execute(&p, &[Value::Exact(Decimal::new(*k, 0)), cv, bv, nv]).unwrap()), 0);
    }
    c.query("COMMIT").unwrap();
}

/// `v`'s octets, read whole over `c` if sent by a locator; none if null.
fn read(c: &mut Connection<TcpStream>, v: &Value) -> Option<Vec<u8>> {
    let Value::Lob(l) = v else { return None };
    let mut all = Vec::new();
    c.lob_reader(l).read_to_end(&mut all).unwrap();
    Some(all)
}

/// `table`'s rows over `c`, by key, each large object's octets.
fn read_back(c: &mut Connection<TcpStream>, table: &str) -> Vec<Given> {
    let Outcome::Rows { rows, .. } = c.query(&format!("SELECT K, C, B, N FROM {table} ORDER BY K")).unwrap() else { panic!("rows") };
    let back = rows
        .iter()
        .map(|r| {
            let Value::Exact(k) = &r[0] else { panic!("{r:?}") };
            let utf8 = |b: Option<Vec<u8>>| b.map(|b| String::from_utf8(b).unwrap());
            (k.units, utf8(read(c, &r[1])), read(c, &r[2]), utf8(read(c, &r[3])))
        })
        .collect();
    c.query("COMMIT").unwrap();
    back
}

/// Verifies: [DESIGN] [RFC 4180 §2] [ISO/IEC 9075-2:2023 §4.37.5, §5.3]
/// RULES.md D975, D976: a table of CLOBs, BLOBs and NCLOBs, empty, short,
/// longer than a reply's body and null, exports as CSV, each large object
/// as its value, a binary one in hexits, read a chunk at a time; imported
/// into another table, every value is as it was; native rows read back
/// so too.
#[test]
fn large_objects_export_as_their_values_and_import_back() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    insert(&mut c, &given());
    let mut csv = Vec::new();
    assert_eq!(c.export("TABLE T", &mut csv, ExportFormat::Csv).unwrap(), 4);
    assert!(csv.starts_with(b"K,C,B,N\r\n1,\"\",\"\",\"\"\r\n2,\"a, \"\"short\"\" one\",0001FEFF,\xC3\xB1\r\n"));
    assert!(!csv.windows(5).any(|w| w == b"LARGE"), "no locator's description");
    c.query("COMMIT").unwrap();
    let dialect = Dialect::csv().with_header().with_max_record(csv.len());
    let mut source = Source::new(csv.as_slice(), dialect);
    assert_eq!(import(&mut c, &mut source, &Import::into("U"), &mut |_| {}).unwrap().rows, 4);
    assert!(read_back(&mut c, "U") == given(), "every value as it was");
    let mut native = Vec::new();
    assert_eq!(c.export("SELECT K, C, B, N FROM T ORDER BY K", &mut native, ExportFormat::Native).unwrap(), 4);
    c.query("COMMIT").unwrap();
    let (columns, back) = read_native(&mut native.as_slice()).unwrap();
    assert_eq!((columns.len(), back.len()), (4, 4));
    let Value::Lob(long) = &back[2][2] else { panic!("a BLOB") };
    assert!(long.inline() == Some(octets(usize::try_from(MAX_BODY).unwrap() + 1000).as_slice()), "held whole");
    c.terminate().unwrap();
}

/// Verifies: [DESIGN] [ISO/IEC 9075-2:2023 §4.37.5] RULES.md D975: a large
/// object within a row or an array, sent by its locator, is read whole
/// and written within its value's text; under autocommit, the
/// transaction an export of large objects began is ended, so no locator
/// stays valid; one active before is left so.
#[test]
fn nested_large_objects_and_the_transaction_they_are_read_in() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let long = "y".repeat(100_000);
    insert(&mut c, &[(1, Some(long.clone()), Some(vec![0xAB; 70_000]), None)]);
    c.set_autocommit(true);
    let mut csv = Vec::new();
    let sql = "SELECT ROW(C, K), ARRAY[B] FROM T";
    assert_eq!(c.export_with(sql, &mut csv, &mut Delimited { header: false, ..Delimited::csv() }).unwrap(), 1);
    let expected = format!("\"ROW('{long}', 1)\",ARRAY[X'{}']\r\n", "AB".repeat(70_000));
    let got = String::from_utf8(csv).unwrap();
    assert!(got == expected, "each read whole within its value");
    assert!(!c.in_transaction(), "the export's own transaction ended");
    c.set_autocommit(false);
    c.query("SELECT K FROM T").unwrap();
    assert!(c.export("TABLE T", &mut Vec::new(), ExportFormat::Csv).is_ok());
    assert!(c.in_transaction(), "a transaction it did not begin left active");
    c.terminate().unwrap();
}

/// A writer that fails once `room` octets are written.
struct Full {
    room: usize,
}

impl Write for Full {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.room {
            return Err(io::Error::other("the disk is full"));
        }
        self.room = self.room.saturating_sub(bytes.len());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Verifies: [DESIGN] RULES.md D975, D976: an export that is no query is
/// refused; one whose write fails stops, its cursor closed and the
/// transaction it began rolled back, the connection still of use; a field
/// for a BLOB column that is not hexits is refused, SQLSTATE 22018.
#[test]
fn exports_and_imports_of_large_objects_refused() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    insert(&mut c, &[(1, Some(text(200_000)), None, None), (2, Some(text(200_000)), None, None)]);
    let not_query = c.export("DELETE FROM T", &mut Vec::new(), ExportFormat::Csv).unwrap_err();
    assert!(not_query.message.contains("an export runs a query"), "{not_query}");
    c.set_autocommit(true);
    let full = c.export("TABLE T", &mut Full { room: 100_000 }, ExportFormat::Csv).unwrap_err();
    assert!(full.message.contains("cannot write the export"), "{full}");
    assert!(!c.in_transaction(), "rolled back");
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["2"], "the connection of use after");
    let mut source = Source::new(&b"9,c,XYZ,n\n"[..], Dialect::csv());
    let bad = import(&mut c, &mut source, &Import::into("U"), &mut |_| {}).unwrap_err();
    let turborel_client::import::Cause::Sql(failure) = &bad.cause else { panic!("{bad}") };
    assert_eq!(state(failure), ("22018", -301), "{bad}");
    c.terminate().unwrap();
}
