//! `\export` of large objects, as a user runs the program (RULES.md D975,
//! D976): a CLOB, an NCLOB and a BLOB written as their values, whole or
//! sent by a locator, and read back by `--import`.

use std::path::Path;
use std::process::{Command, Output};

use turborel::db::Database;
use turborel::value::lob::Lob;
use turborel::value::{Decimal, Value};

use crate::common::{Scratch, start};

/// The columns of T and U.
const COLUMNS: &str = "(K INTEGER, C CLOB, B BLOB, N NCLOB)";

/// `turborel` run with `args`.
fn program(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_turborel")).args(args).env_remove(turborel_cli::PASSWORD_VARIABLE).output().unwrap()
}

/// A database whose schema X holds T, of a row of empty values, one of
/// short ones, one of values longer than a server sends whole, and one of
/// nulls, and U, empty, of the same columns.
fn database(long: &str) -> Database {
    let db = Database::new();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T {COLUMNS} CREATE TABLE U {COLUMNS}")).unwrap();
    let session = db.session("X");
    let insert = session.prepare("INSERT INTO T VALUES (?, ?, ?, ?)").unwrap();
    let octets: Vec<u8> = (0..long.len()).map(|i| u8::try_from(i % 256).unwrap()).collect();
    let rows = [
        (1, Some(("", &b""[..], ""))),
        (2, Some(("one, \"two\"", &[0xCA, 0xFE][..], "ñ"))),
        (3, Some((long, octets.as_slice(), long))),
        (4, None),
    ];
    for (k, values) in rows {
        let lobs = values.map_or([Value::Null, Value::Null, Value::Null], |(clob, blob, nclob)| {
            [Value::Lob(Lob::of_text(clob)), Value::Lob(Lob::of_octets(blob)), Value::Lob(Lob::of_text(nclob))]
        });
        let [clob, blob, nclob] = lobs;
        session.execute_prepared(&insert, &[Value::Exact(Decimal::new(k, 0)), clob, blob, nclob]).unwrap();
    }
    session.commit().unwrap();
    db
}

/// The one number `sql` counts in `db`.
fn count(db: &Database, sql: &str) -> String {
    let s = db.session("X");
    let row = s.open_cursor(sql).unwrap().fetch().unwrap().unwrap();
    row[0].to_string()
}

/// `path`'s text.
fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// Verifies: [DESIGN] RULES.md D216, D975, D976: `\export` in a script the
/// program runs writes each large object as its value, a binary one in
/// hexits, one longer than a server sends whole read by its locator, as
/// CSV, as text and as native rows; `--import` reads the CSV back into
/// another table, every value equal to its original; text refuses a value
/// it cannot hold.
#[test]
fn large_objects_are_exported_and_imported_by_the_program() {
    let long = "lé\"ng,".repeat(60_000);
    let db = database(&long);
    let server = start(&db, &[]);
    let port = server.address.port().to_string();
    let scratch = Scratch::new("cli-exported");
    let (csv, text, native, script) = (scratch.path("t.csv"), scratch.path("t.txt"), scratch.path("t.native"), scratch.path("e.sql"));
    let commands = format!(
        "\\export T {} header\n\\export (SELECT K, B FROM T WHERE K < 4 ORDER BY K) {} text null -\n\\export T {} native\n",
        csv.display(),
        text.display(),
        native.display()
    );
    std::fs::write(&script, commands).unwrap();
    let exported = program(&["-p", &port, "-U", "X", "-f", &script.display().to_string()]);
    assert!(exported.status.success(), "{}", String::from_utf8_lossy(&exported.stderr));
    assert_eq!(String::from_utf8(exported.stdout).unwrap(), "OK, 4 rows exported\nOK, 3 rows exported\nOK, 4 rows exported\n");
    let written = read(&csv);
    assert!(
        written.starts_with("K,C,B,N\n1,\"\",\"\",\"\"\n2,\"one, \"\"two\"\"\",CAFE,ñ\n3,\""),
        "{}",
        written.chars().take(80).collect::<String>()
    );
    assert!(written.ends_with("\"\n4,,,\n") && !written.contains("LARGE OBJECT"));
    assert!(written.contains(&long.replace('"', "\"\"")), "the long CLOB whole");
    let hexits = read(&text);
    assert!(hexits.starts_with("1\t\n2\tCAFE\n3\t000102"), "{}", hexits.chars().take(20).collect::<String>());
    let (_, rows) = turborel_client::export::read_native(&mut std::fs::File::open(&native).unwrap()).unwrap();
    let Value::Lob(back) = &rows[2][1] else { panic!("{:?}", rows[2]) };
    assert_eq!(back.inline(), Some(long.as_bytes()), "native rows hold it whole");
    let imported = program(&["-p", &port, "-U", "X", "--import", &csv.display().to_string(), "--table", "U", "--header"]);
    assert!(imported.status.success(), "{}", String::from_utf8_lossy(&imported.stderr));
    let same = "SELECT COUNT(*) FROM T JOIN U ON T.K = U.K WHERE T.C = U.C AND T.B = U.B AND T.N = U.N";
    assert_eq!(count(&db, same), "3", "each value as it was");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM U WHERE K = 4 AND C IS NULL AND B IS NULL AND N IS NULL"), "1");
    std::fs::write(&script, format!("\\export (SELECT C FROM T WHERE K = 3) {} text delimiter ,\n", text.display())).unwrap();
    let refused = program(&["-p", &port, "-U", "X", "-f", &script.display().to_string()]);
    assert!(!refused.status.success());
    assert!(String::from_utf8(refused.stderr).unwrap().contains("text cannot hold it"));
}
