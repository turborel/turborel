//! Large objects over a connection [ISO/IEC 9075-2:2023 §4.37.5] (RULES.md
//! D45, D79): one at most SERVER.LOB_INLINE_BYTES long sent whole, a
//! longer one by its locator and read by ranges or as a stream; a value
//! the client writes a piece at a time sent by its locator; a result that
//! sends a locator keeping the transaction; the locator freed and held;
//! and an import's long field sent so.

use std::io::{Read, Write};
use std::net::TcpStream;

use turborel::db::Database;
use turborel::value::lob::{Lob, LobKind};
use turborel::value::{Decimal, Value};
use turborel_client::connection::{Connection, Outcome};
use turborel_client::import::{Delimited, Dialect, Import, import};
use turborel_server::settings::LOB_INLINE_BYTES;

use crate::common::{done, rows, start, state};

/// A database whose schema X holds T, of a key K and a CLOB C.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER, C CLOB)").unwrap();
    db
}

/// A text of `n` characters, some of two octets.
fn text(n: usize) -> String {
    (0..n).map(|i| if i % 7 == 0 { 'é' } else { 'w' }).collect()
}

/// Row `k`'s C, as a query over `c` sends it.
fn clob(c: &mut Connection<TcpStream>, k: i64) -> Lob {
    let Outcome::Rows { rows, .. } = c.query(&format!("SELECT C FROM T WHERE K = {k}")).unwrap() else { panic!("no rows") };
    let Some(Value::Lob(l)) = rows.first().and_then(|r| r.first()) else { panic!("{rows:?}") };
    l.clone()
}

/// Inserts `lob` as row `k` over `c`.
fn insert(c: &mut Connection<TcpStream>, k: i128, lob: Lob) {
    let p = c.prepare("INSERT INTO T VALUES (?, ?)").unwrap();
    assert_eq!(done(&c.execute(&p, &[Value::Exact(Decimal::new(k, 0)), Value::Lob(lob)]).unwrap()), 0);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.37.5]; [DESIGN] RULES.md D79:
/// a value the client writes a piece at a time is inserted by its locator; read
/// back, one of at most SERVER.LOB_INLINE_BYTES comes whole, a longer one by a
/// locator, read by ranges of octets and of characters and as a stream, valid
/// until the transaction ends (SQLSTATE 0F001 after).
#[test]
fn a_long_value_goes_by_its_locator() {
    let db = database();
    let server = start(&db, &[(LOB_INLINE_BYTES, 1000)]);
    let mut c = server.connect("X");
    let long = text(300_000);
    let mut writer = c.create_lob(LobKind::Character).unwrap();
    for piece in long.as_bytes().chunks(70_001) {
        writer.write_all(piece).unwrap();
    }
    let made = writer.finish().unwrap();
    assert_eq!((made.octets(), made.chars()), (u64::try_from(long.len()).unwrap(), 300_000));
    insert(&mut c, 1, made);
    insert(&mut c, 2, Lob::of_text("short"));
    c.query("COMMIT").unwrap();
    assert_eq!(clob(&mut c, 2).inline(), Some(&b"short"[..]), "sent whole");
    let l = clob(&mut c, 1);
    assert!(l.locator().is_some() && l.chars() == 300_000, "sent by its locator");
    assert_eq!(c.read_lob(&l, (0, 3), false).unwrap(), "éw".as_bytes());
    let tail: String = long.chars().skip(299_998).collect();
    assert_eq!(c.read_lob(&l, (299_998, 10), true).unwrap(), tail.as_bytes());
    let mut all = Vec::new();
    c.lob_reader(&l).read_to_end(&mut all).unwrap();
    assert_eq!(all, long.as_bytes(), "streamed a chunk at a time");
    c.query("COMMIT").unwrap();
    assert_eq!(state(&c.read_lob(&l, (0, 1), false).unwrap_err()), ("0F001", -301));
    c.terminate().unwrap();
}

/// Verifies: [DESIGN] RULES.md D45, D79: under autocommit, a statement
/// whose result sent a locator keeps its transaction, so the locator is
/// read; one that sent none ends it. A held locator is valid past the
/// commit \[§14.18\]; a freed one is not \[§14.17\].
#[test]
fn a_locator_keeps_its_transaction_until_freed_or_held() {
    let db = database();
    let server = start(&db, &[(LOB_INLINE_BYTES, 16)]);
    let mut c = server.connect("X");
    insert(&mut c, 1, Lob::of_text(&text(5_000)));
    c.query("COMMIT").unwrap();
    c.set_autocommit(true);
    assert_eq!(rows(&c.query("SELECT K FROM T").unwrap()), ["1"]);
    assert!(!c.in_transaction(), "no locator: ended");
    let l = clob(&mut c, 1);
    assert!(c.in_transaction(), "a locator: kept");
    assert_eq!(c.read_lob(&l, (0, 2), true).unwrap(), "éw".as_bytes());
    done(&c.hold_lob(&l).map(Outcome::Done).unwrap());
    c.set_autocommit(false);
    c.query("COMMIT").unwrap();
    assert_eq!(c.read_lob(&l, (4_998, 5), true).unwrap(), text(5_000).chars().skip(4_998).collect::<String>().as_bytes(), "held");
    c.free_lob(&l).unwrap();
    assert_eq!(state(&c.read_lob(&l, (0, 1), false).unwrap_err()).0, "0F001", "freed");
    assert_eq!(state(&c.free_lob(&l).unwrap_err()).0, "0F001");
    c.terminate().unwrap();
}

/// Verifies: [DESIGN] RULES.md D59, D79: an import's field longer than a
/// value sent whole, for a CLOB column, is sent ahead by a locator, a
/// piece at a time, and inserted as read.
#[test]
fn an_import_sends_a_long_field_by_a_locator() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let long = text(200_000);
    let csv = format!("1,{long}\n2,short\n");
    let mut source = Delimited::new(csv.as_bytes(), Dialect::csv().with_max_record(1 << 20));
    assert_eq!(import(&mut c, &mut source, &Import::into("T"), &mut |_| {}).unwrap().rows, 2);
    assert_eq!(rows(&c.query("SELECT K, CHAR_LENGTH(C) FROM T ORDER BY K").unwrap()), ["1,200000", "2,5"]);
    let l = clob(&mut c, 1);
    let mut all = Vec::new();
    c.lob_reader(&l).read_to_end(&mut all).unwrap();
    assert_eq!(all, long.as_bytes());
    c.terminate().unwrap();
}

/// Verifies: [DESIGN] RULES.md D45, D79: a value a result sends whole is
/// read from itself, by ranges of octets or characters, past its end
/// empty, and streamed; a value the server did not send, or one sent
/// whole, has no locator to read, free or hold (SQLCODE -301); a BLOB
/// written in pieces longer than an Append, then flushed, is inserted and
/// read back whole.
#[test]
fn values_sent_whole_and_binary_values_written_in_pieces() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE B (K INTEGER, B BLOB)").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let whole = Lob::of_text("aé€😀b");
    assert_eq!(c.read_lob(&whole, (1, 2), true).unwrap(), "é€".as_bytes(), "characters");
    assert_eq!(c.read_lob(&whole, (1, 2), false).unwrap(), "é".as_bytes(), "octets");
    assert_eq!(c.read_lob(&whole, (100, 2), false).unwrap(), b"", "past the end");
    let bytes = Lob::of_octets(&[1, 2, 3]);
    assert_eq!(c.read_lob(&bytes, (1, 5), true).unwrap(), [2, 3], "a binary one's characters are its octets");
    let mut streamed = Vec::new();
    c.lob_reader(&whole).read_to_end(&mut streamed).unwrap();
    assert_eq!(streamed, "aé€😀b".as_bytes());
    assert_eq!(c.lob_reader(&whole).read(&mut []).unwrap(), 0, "nothing asked");
    let unsent = Lob::of_reference(LobKind::Character, 1, (10, 10));
    assert_eq!(state(&c.read_lob(&unsent, (0, 1), false).unwrap_err()).1, -301);
    assert_eq!(state(&c.free_lob(&whole).unwrap_err()).1, -301);
    assert_eq!(state(&c.hold_lob(&whole).unwrap_err()).1, -301);
    let octets: Vec<u8> = (0..600_000_u32).map(|i| u8::try_from(i % 251).unwrap()).collect();
    let mut writer = c.create_lob(LobKind::Binary).unwrap();
    writer.write_all(&octets[..300_000]).unwrap();
    writer.flush().unwrap();
    writer.flush().unwrap();
    writer.write_all(&octets[300_000..]).unwrap();
    let made = writer.finish().unwrap();
    assert_eq!((made.octets(), made.chars()), (600_000, 600_000));
    let p = c.prepare("INSERT INTO B VALUES (1, ?)").unwrap();
    assert_eq!(done(&c.execute(&p, &[Value::Lob(made)]).unwrap()), 0);
    let Outcome::Rows { rows, .. } = c.query("SELECT B FROM B").unwrap() else { panic!("no rows") };
    let Some(Value::Lob(read)) = rows.first().and_then(|r| r.first()) else { panic!("{rows:?}") };
    let mut all = Vec::new();
    c.lob_reader(read).read_to_end(&mut all).unwrap();
    assert_eq!(all, octets);
    c.terminate().unwrap();
}

/// Verifies: [ISO/IEC 9075-2:2023 §20.7 GR5a xix 1 A, §14.11 SR16]
/// RULES.md D390: over a connection, a 4 MiB BLOB and an 80 KB CLOB bound
/// to the `?`s of the first row of an INSERT's VALUES, beside a second row
/// of X'0102' and 'short', are stored whole and read back octet for octet.
/// Regression test: they were cut to two octets and five characters.
#[test]
fn bound_values_beside_shorter_literals_store_whole() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE SCANS (ID INTEGER, B BLOB(10M), C CLOB(1M))").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let blob: Vec<u8> = (0..4_u32 << 20).map(|i| u8::try_from(i % 251).unwrap()).collect();
    let clob = text(80_000);
    let p = c.prepare("INSERT INTO SCANS VALUES (1, ?, ?), (2, X'0102', 'short')").unwrap();
    assert_eq!(done(&c.execute(&p, &[Value::Lob(Lob::of_octets(&blob)), Value::Lob(Lob::of_text(&clob))]).unwrap()), 0);
    let Outcome::Rows { rows, .. } = c.query("SELECT B, C FROM SCANS ORDER BY ID").unwrap() else { panic!("no rows") };
    let mut read = Vec::new();
    for v in rows.iter().flatten() {
        let Value::Lob(l) = v else { panic!("{v:?}") };
        let mut all = Vec::new();
        c.lob_reader(l).read_to_end(&mut all).unwrap();
        read.push(all);
    }
    assert_eq!(read, [blob, clob.into_bytes(), vec![1, 2], b"short".to_vec()]);
    c.terminate().unwrap();
}
