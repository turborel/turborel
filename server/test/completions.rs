//! What a completion carries over a real connection, as the client was
//! granted (RULES.md D186, D187, D197 to D199): the rows a statement
//! counted, as GET DIAGNOSTICS ROW_COUNT does (issue 5); the identity
//! values an INSERT generated (issue 6); a transaction ended as its cursor
//! does (issue 8); every condition a statement raised (issue 10); and an
//! error's structured diagnostics.

use turborel::db::Database;
use turborel::params::Params;
use turborel::value::{DataType, Value};
use turborel_client::told::Offer;
use turborel_protocol::capability::{CONDITIONS, ROW_COUNTS};

use crate::common::{rows, start, state};
use crate::negotiated::offering;

/// How long the holder of a row keeps it once the other connection waits
/// for it: the least that connection's statement waits for the lock.
const LOCK_HELD: std::time::Duration = std::time::Duration::from_millis(100);

/// The looks at CONNECTIONS for a connection waiting on a lock, 5 ms
/// apart, before a run that never gets there gives up.
const LOOKS: usize = 2_000;

/// Whether a connection of X's to `db` waits on a lock, looked at until
/// one does or `LOOKS` looks have passed.
fn waits_on_a_lock(db: &Database) -> bool {
    let looking = db.session("X");
    let sql = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.CONNECTIONS WHERE WAITING_ON = 'LOCK'";
    (0..LOOKS).any(|_| {
        let seen = turborel_test_support::database::rows(&looking, sql).unwrap();
        looking.rollback().unwrap();
        seen == ["1"] || {
            std::thread::sleep(std::time::Duration::from_millis(5));
            false
        }
    })
}

/// A database whose schema X holds T, of an INTEGER K, and I, of an
/// identity column ID and a V.
fn database() -> Database {
    let db = Database::new();
    let tables = "CREATE TABLE T (K INTEGER) CREATE TABLE I (ID INTEGER GENERATED ALWAYS AS IDENTITY, V INTEGER)";
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION X {tables}")).unwrap();
    db
}

/// The ROW_COUNT GET DIAGNOSTICS gives after `sql`, run by an embedded
/// session of X of `db`.
fn diagnosed(db: &Database, sql: &str) -> u64 {
    let session = db.session("X");
    session.direct(sql).unwrap();
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: turborel::value::Spelled::Integer };
    let mut params = Params::new().declare("N", integer, Value::Null);
    session.execute("GET DIAGNOSTICS N = ROW_COUNT", &mut params).unwrap();
    params.get("N").unwrap().value.to_string().parse().unwrap()
}

/// Verifies: [DESIGN] [ISO/IEC 9075:1992 §18.1 GR1e] RULES.md D197, issue 5:
/// a completion carries the rows its INSERT, UPDATE, DELETE or MERGE
/// changed, the very ROW_COUNT GET DIAGNOSTICS gives for it, under
/// autocommit too; a query's, the rows it returned, once all were; a client
/// not granted `row-counts` is told none.
#[test]
fn a_completion_counts_rows_as_get_diagnostics_does() {
    let (db, twin) = (database(), database());
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let statements = [
        "INSERT INTO T VALUES (1), (2), (3)",
        "UPDATE T SET K = K + 1 WHERE K > 1",
        "MERGE INTO T USING (VALUES (4), (9)) AS S (N) ON T.K = S.N WHEN MATCHED THEN DELETE WHEN NOT MATCHED THEN INSERT VALUES (S.N)",
        "DELETE FROM T WHERE K = 7",
        "DELETE FROM T",
    ];
    for (n, sql) in statements.iter().enumerate() {
        c.set_autocommit(n % 2 == 0);
        c.query(sql).unwrap();
        assert_eq!(c.rows_affected(), Some(diagnosed(&twin, sql)), "{sql}");
    }
    c.query("INSERT INTO T VALUES (5), (6)").unwrap();
    c.query("SELECT K FROM T").unwrap();
    assert_eq!(c.rows_affected(), Some(2), "a query's rows returned");
    let query = c.prepare("SELECT K FROM T").unwrap();
    c.open_cursor(&query, &[], 1).unwrap();
    c.fetch(&query, 0).unwrap();
    assert_eq!(c.rows_affected(), Some(2), "a cursor's rows, all of its fetches'");
    let mut plain = offering(&server, Offer { capabilities: vec![], ..Offer::none() });
    plain.query("INSERT INTO T VALUES (8)").unwrap();
    assert_eq!(plain.rows_affected(), None);
    let counted = offering(&server, Offer { capabilities: vec![ROW_COUNTS.into()], ..Offer::none() });
    assert!(counted.granted().has(ROW_COUNTS));
}

/// Verifies: [DESIGN] [ISO/IEC 9075-2:2023 §11.4 SR10, §15.10 GR] RULES.md
/// D198, issue 6: an INSERT's completion carries the identity values it
/// generated, one or many in the order of its rows, by VALUES or by a
/// query; of a table with a trigger whose INSERT generates values of its
/// own, only the statement's; none of a statement that generated none.
#[test]
fn an_insert_tells_the_identity_values_it_generated() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION L CREATE TABLE LOG (N INTEGER GENERATED ALWAYS AS IDENTITY (START WITH 100), K INTEGER)")
        .unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    c.query("INSERT INTO I (V) VALUES (10)").unwrap();
    assert_eq!(c.last_generated(), ["1"]);
    c.query("INSERT INTO I (V) VALUES (20), (30), (40)").unwrap();
    assert_eq!(c.last_generated(), ["2", "3", "4"]);
    c.query("INSERT INTO T VALUES (1), (2)").unwrap();
    assert_eq!(c.last_generated().len(), 0, "T has no identity column");
    c.query("INSERT INTO I (V) SELECT K FROM T ORDER BY K").unwrap();
    assert_eq!(c.last_generated(), ["5", "6"]);
    c.query("COMMIT").unwrap();
    let trigger = "CREATE TRIGGER X.LOGGED AFTER INSERT ON X.I REFERENCING NEW ROW AS R FOR EACH ROW INSERT INTO X.T VALUES (R.V)";
    c.query(trigger).unwrap();
    c.query("CREATE TABLE X.J (ID INTEGER GENERATED ALWAYS AS IDENTITY (START WITH 50), V INTEGER)").unwrap();
    c.query("CREATE TRIGGER X.TWICE AFTER INSERT ON X.T REFERENCING NEW ROW AS R FOR EACH ROW INSERT INTO X.J (V) VALUES (R.K)").unwrap();
    c.query("INSERT INTO I (V) VALUES (70)").unwrap();
    assert_eq!(c.last_generated(), ["7"], "not J's, which a trigger's INSERT generated");
    c.query("UPDATE I SET V = 0").unwrap();
    assert_eq!(c.last_generated().len(), 0);
}

/// Verifies: [DESIGN] RULES.md D199, issue 8: granted `fetch-ends`, an
/// Execute that ends its transaction and leaves its cursor open commits it
/// once a Fetch reads the last row, or a Close closes the cursor early, and
/// rolls it back if its rows fail; not granted, the transaction stays
/// active after the last Fetch.
#[test]
fn a_cursor_ends_its_transaction_as_it_ends() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    c.query("INSERT INTO T VALUES (1), (2), (3)").unwrap();
    c.query("COMMIT").unwrap();
    c.set_autocommit(true);
    let query = c.prepare("SELECT K FROM T ORDER BY K").unwrap();
    assert!(c.open_cursor(&query, &[], 1).unwrap().more);
    assert!(c.in_transaction());
    assert!(c.fetch(&query, 1).unwrap().more && c.in_transaction());
    let last = c.fetch(&query, 0).unwrap();
    assert!(!last.more && !c.in_transaction(), "committed as its last row was read");
    c.open_cursor(&query, &[], 1).unwrap();
    c.close(&query).unwrap();
    assert!(!c.in_transaction(), "committed as it was closed");
    c.query("INSERT INTO T VALUES (3)").unwrap();
    let divided = c.prepare("SELECT 10 / (K - 3) FROM T").unwrap();
    c.query("INSERT INTO T VALUES (4)").unwrap();
    assert_eq!(state(&c.open_cursor(&divided, &[], 1).unwrap_err()).0, "22012");
    assert!(!c.in_transaction(), "rolled back as its rows failed");
    let mut old = offering(&server, Offer::none());
    old.set_autocommit(true);
    let query = old.prepare("SELECT K FROM T").unwrap();
    old.open_cursor(&query, &[], 1).unwrap();
    old.fetch(&query, 0).unwrap();
    assert!(old.in_transaction(), "not granted: left active");
}

/// Verifies: [DESIGN] [ISO/IEC 9075-4:2006 draft §17.2, §17.3 GR6a iii]
/// RULES.md D187, issue 10: granted `conditions`, a completion carries each
/// condition its statement raised, in order, each with the fields of an
/// error: the warnings its SIGNALs raised with their messages, and the
/// condition of class 02 it completed with; no notice besides; their
/// diagnostics only to a client granted them, as an error's.
#[test]
fn a_completion_carries_every_condition() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let sql = "BEGIN SIGNAL SQLSTATE '01H01' SET MESSAGE_TEXT = 'first'; SIGNAL SQLSTATE '01H02' SET MESSAGE_TEXT = 'second'; \
               SIGNAL SQLSTATE '02H01' SET MESSAGE_TEXT = 'none left'; END";
    c.query(sql).unwrap();
    let told: Vec<(&str, i32, &str)> = c.conditions().iter().map(|f| (f.sqlstate.as_str(), f.sqlcode, f.message.as_str())).collect();
    assert_eq!(told, [("01H01", 0, "first"), ("01H02", 0, "second"), ("02H01", 100, "none left")]);
    assert!(c.conditions().iter().all(|f| f.diagnostics.as_ref().and_then(|d| d.severity.as_deref()) == Some("WARNING")));
    assert_eq!(c.take_notices().len(), 0, "in the completion, not as notices");
    c.query("SELECT SUM(K) FROM T").unwrap();
    assert_eq!(c.conditions().len(), 0, "no null eliminated of an empty table");
    c.query("INSERT INTO T VALUES (NULL), (1)").unwrap();
    c.query("SELECT SUM(K) FROM T").unwrap();
    assert_eq!(c.conditions().iter().map(|f| f.sqlstate.as_str()).collect::<Vec<_>>(), ["01003"]);
    let mut bare = offering(&server, Offer { capabilities: vec![CONDITIONS.into()], ..Offer::none() });
    bare.query(sql).unwrap();
    assert_eq!(bare.conditions().len(), 3);
    assert!(bare.conditions().iter().all(|f| f.diagnostics.is_none()), "not granted diagnostics");
}

/// Verifies: [DESIGN] [ISO/IEC 9075:1992 §18.1 GR3f, GR3g] RULES.md D186: an
/// error carries its severity and what it is about: a duplicate key its
/// constraint, table and schema, and the key in its detail; a value its
/// column refuses, the column's type; a client not granted `diagnostics`
/// reads none.
#[test]
fn an_error_says_what_it_is_about() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE U (K INTEGER CONSTRAINT UK UNIQUE, S CHARACTER(2))").unwrap();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    c.query("INSERT INTO U VALUES (1, 'a')").unwrap();
    let duplicate = c.query("INSERT INTO U VALUES (1, 'b')").unwrap_err();
    let d = duplicate.diagnostics.as_deref().unwrap();
    assert_eq!(d.severity.as_deref(), Some("ERROR"));
    assert_eq!((d.schema.as_deref(), d.table.as_deref(), d.constraint.as_deref()), (Some("X"), Some("U"), Some("UK")));
    assert_eq!(d.detail.as_deref(), Some("the key (1) is held by another row"));
    let long = c.query("INSERT INTO U VALUES (2, 'abc')").unwrap_err();
    assert_eq!(long.sqlstate, "22001");
    assert_eq!(long.diagnostics.as_deref().and_then(|d| d.data_type.as_deref()), Some("CHARACTER(2)"));
    let mut plain = offering(&server, Offer::none());
    assert!(plain.query("INSERT INTO U VALUES (1, 'c')").unwrap_err().diagnostics.is_none());
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM U").unwrap()), ["1"]);
}

/// Verifies: [DESIGN] RULES.md D240: granted `row-counts`, a completion
/// carries the server's time for its statement, no more than the client
/// waited for it; a cursor's summed over its requests and told by the
/// Fetch that completes it, no more than the client waited for those
/// requests, so not the time between them; the time waited for a lock
/// among it, at least as long as the lock was held once it waited, and
/// none of a statement that waited for none; none of a Close; not
/// granted, none.
#[test]
fn a_completion_tells_the_servers_time() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    c.query("INSERT INTO T VALUES (1), (2)").unwrap();
    c.query("COMMIT").unwrap();
    let began = std::time::Instant::now();
    c.query("SELECT COUNT(*) FROM T AS A, T AS B").unwrap();
    let waited = u64::try_from(began.elapsed().as_micros()).unwrap();
    let time = c.server_time().unwrap();
    assert!(time.total > 0 && time.total <= waited, "{time:?}, waited {waited} us");
    assert_eq!(time.locks, 0, "no lock waited for");
    let query = c.prepare("SELECT K FROM T").unwrap();
    let began = std::time::Instant::now();
    c.open_cursor(&query, &[], 1).unwrap();
    let opening = began.elapsed();
    std::thread::sleep(std::time::Duration::from_millis(30));
    let began = std::time::Instant::now();
    c.fetch(&query, 0).unwrap();
    let asked = u64::try_from((opening + began.elapsed()).as_micros()).unwrap();
    let cursor = c.server_time().unwrap();
    assert!(cursor.total <= asked, "the time between requests is not the server's: {cursor:?}, asked {asked} us");
    let mut holder = server.connect("X");
    holder.query("UPDATE T SET K = K WHERE K = 1").unwrap();
    let watched = db.clone();
    let releasing = std::thread::spawn(move || {
        let waited = waits_on_a_lock(&watched);
        std::thread::sleep(LOCK_HELD);
        holder.query("COMMIT").unwrap();
        waited
    });
    c.query("UPDATE T SET K = K WHERE K = 1").unwrap();
    assert!(releasing.join().unwrap(), "the second UPDATE waits for the row");
    let blocked = c.server_time().unwrap();
    let held = u64::try_from(LOCK_HELD.as_micros()).unwrap();
    assert!(blocked.locks >= held && blocked.locks <= blocked.total, "{blocked:?}");
    c.open_cursor(&query, &[], 1).unwrap();
    c.close(&query).unwrap();
    assert_eq!(c.server_time(), None, "a Close is no statement");
    let mut plain = offering(&server, Offer::none());
    plain.query("SELECT K FROM T").unwrap();
    assert_eq!(plain.server_time(), None);
}
