//! Users and the database's owner (RULES.md D43): kept with the
//! definitions, managed by the host program's API and, in SQL, by the
//! owner alone; their verifiers made by a credentials plug-in.

use turborel::db::{Credentials, Database, OpenOptions, Opened};
use turborel::error::{Code, Result};
use turborel::lexer::authorization_identifier;
use turborel::params::Params;

use crate::common::{Kept, Scratch, after_crash, exec, rows};

/// A credentials plug-in, `reversed`, whose verifier is the user's name, a
/// colon, and the password backwards: plain to read, so a test sees what
/// was kept. A test-local plug-in (RULES.md D6); no real one is so weak.
struct Reversed;

impl Credentials for Reversed {
    fn name(&self) -> &'static str {
        "reversed"
    }
    fn verifier(&self, user: &str, password: &str) -> Result<Vec<u8>> {
        Ok(format!("{user}:{}", password.chars().rev().collect::<String>()).into_bytes())
    }
}

fn code<T>(r: Result<T>) -> Option<Code> {
    r.err().map(|e| e.code)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4; Annex B, item 41];
/// [DESIGN] RULES.md D43: a user is created with its verifier, which it
/// keeps until given another, and is dropped; a name is a user once. Each
/// is refused (SQLCODE -103): a user twice, a user not there, a name empty
/// or longer than an identifier. The owner is a user, the first one
/// created until another is made it; it cannot be dropped until another
/// owns the database. A name is a user while it is one (`is_user`). The
/// sink hears of each change, never of a verifier.
#[test]
fn users_are_kept_with_their_verifiers() {
    let db = Database::new();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone()));
    assert_eq!((db.users().unwrap(), db.owner().unwrap()), (Vec::<String>::new(), None));
    db.create_user("ANN", b"one").unwrap();
    db.create_user("bob", b"").unwrap();
    assert_eq!(db.users().unwrap(), ["ANN", "bob"]);
    assert_eq!(db.owner().unwrap().as_deref(), Some("ANN"), "the first user owns the database");
    assert_eq!(db.verifier("ANN").unwrap().as_deref(), Some(&b"one"[..]));
    db.set_verifier("ANN", b"two").unwrap();
    assert_eq!(db.verifier("ANN").unwrap().as_deref(), Some(&b"two"[..]));
    assert_eq!(db.verifier("CAROL").unwrap(), None);
    assert_eq!(code(db.create_user("ANN", b"")), Some(Code::SyntaxRule), "twice");
    assert_eq!(code(db.set_verifier("CAROL", b"")), Some(Code::SyntaxRule), "no such user");
    assert_eq!(code(db.drop_user("CAROL")), Some(Code::SyntaxRule), "no such user");
    assert_eq!(code(db.create_user("", b"")), Some(Code::SyntaxRule), "empty");
    assert_eq!(code(db.create_user(&"A".repeat(129), b"")), Some(Code::SyntaxRule), "too long");
    db.create_user(&"A".repeat(128), b"").unwrap();
    assert_eq!(code(db.set_owner("CAROL")), Some(Code::SyntaxRule), "an owner is a user");
    db.set_owner("ANN").unwrap();
    assert_eq!(db.owner().unwrap().as_deref(), Some("ANN"));
    assert_eq!(code(db.drop_user("ANN")), Some(Code::SyntaxRule), "the owner");
    db.set_owner("bob").unwrap();
    assert!(db.is_user("ANN").unwrap());
    db.drop_user("ANN").unwrap();
    assert!(!db.users().unwrap().contains(&"ANN".to_owned()));
    assert!(!db.is_user("ANN").unwrap() && !db.is_user("CAROL").unwrap() && db.is_user("bob").unwrap());
    let events = kept.taken().into_iter().map(|(_, e)| e).collect::<Vec<_>>().join("\n");
    assert!(events.contains(r#"UserChanged { name: "ANN", dropped: false }"#), "{events}");
    assert!(events.contains(r#"UserChanged { name: "ANN", dropped: true }"#), "{events}");
    assert!(events.contains(r#"OwnerChanged { name: "ANN" }"#), "the first user, made the owner: {events}");
    assert!(events.contains(r#"OwnerChanged { name: "bob" }"#), "{events}");
    assert!(!events.contains("two"), "no verifier is told");
}

/// Verifies: [DESIGN] RULES.md D25, D43: users and the owner are kept with
/// the definitions: after a crash, those made before a checkpoint come back
/// from it, those made after from the log; and all from a checkpoint taken
/// once recovered, after a second crash.
#[test]
fn users_survive_a_crash() {
    let scratch = Scratch::new("users-crash");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        let opened = db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        (db, opened)
    };
    let (db, _) = open();
    db.create_user("ANN", b"checkpointed").unwrap();
    db.create_user("BOB", b"dropped").unwrap();
    db.set_owner("ANN").unwrap();
    db.checkpoint().unwrap();
    db.create_user("CAROL", b"logged").unwrap();
    db.set_owner("CAROL").unwrap();
    db.drop_user("BOB").unwrap();
    db.set_verifier("ANN", b"changed").unwrap();
    drop(db);
    let (db, opened) = open();
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(db.users().unwrap(), ["ANN", "CAROL"]);
    assert_eq!(db.verifier("ANN").unwrap().as_deref(), Some(&b"changed"[..]));
    assert_eq!(db.owner().unwrap().as_deref(), Some("CAROL"));
    db.checkpoint().unwrap();
    drop(db);
    let (db, _) = open();
    assert_eq!(
        (db.users().unwrap(), db.owner().unwrap().as_deref().map(str::to_owned)),
        (vec!["ANN".to_owned(), "CAROL".to_owned()], Some("CAROL".to_owned()))
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4]; [DESIGN] RULES.md D43:
/// in SQL, the owner creates a user with a password, which the credentials
/// plug-in makes a verifier of, gives it another, and drops it; the
/// statements' `<authorization identifier>`s are identifiers, a regular one
/// folded to upper case, a delimited one as written; WITH is optional; a
/// refusal is placed at the identifier.
#[test]
fn the_owner_manages_users_in_sql() {
    let db = Database::new();
    db.set_credentials(Box::new(Reversed));
    db.create_user("OWNER", b"").unwrap();
    db.set_owner("OWNER").unwrap();
    let s = db.session("OWNER");
    exec(&s, "CREATE USER ann PASSWORD 'secret'").unwrap();
    assert_eq!(db.verifier("ANN").unwrap().as_deref(), Some(&b"ANN:terces"[..]));
    exec(&s, "ALTER USER Ann WITH PASSWORD 'it''s'").unwrap();
    assert_eq!(db.verifier("ANN").unwrap().as_deref(), Some(&b"ANN:s'ti"[..]));
    exec(&s, r#"CREATE USER "bob" WITH PASSWORD ''"#).unwrap();
    assert_eq!(db.verifier("bob").unwrap().as_deref(), Some(&b"bob:"[..]));
    exec(&s, "DROP USER ann").unwrap();
    assert_eq!(db.users().unwrap(), ["OWNER", "bob"]);
    assert_eq!(exec(&s, "CREATE USER bob2 PASSWORD 'x'").map(|_| ()), Ok(()));
    assert_eq!(exec(&s, r#"CREATE USER "bob" PASSWORD 'x'"#), Err(Code::SyntaxRule), "a user twice");
    assert_eq!(exec(&s, "DROP USER owner"), Err(Code::SyntaxRule), "the owner");
    assert_eq!(s.run("ALTER USER nope PASSWORD 'x'").unwrap_err().pos, Some(11), "at the identifier");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4]; [DESIGN] RULES.md D43,
/// R-ERR-1: a user statement is refused, nothing done: by any but the
/// owner (SQLCODE -104), or when there is none; in a transaction (-917);
/// with no credentials plug-in to keep a password (-910); and as a Format
/// broken (-102) without PASSWORD, without a `<character string literal>`
/// after it, without an identifier, or with more after its end.
#[test]
fn user_statements_are_refused_saying_why() {
    let db = Database::new();
    let s = db.session("ANN");
    assert_eq!(exec(&s, "CREATE USER bob PASSWORD 'x'"), Err(Code::Privilege), "no owner");
    db.create_user("ANN", b"").unwrap();
    db.set_owner("ANN").unwrap();
    assert_eq!(exec(&s, "CREATE USER bob PASSWORD 'x'"), Err(Code::Api), "no credentials plug-in");
    db.set_credentials(Box::new(Reversed));
    assert_eq!(exec(&db.session("BOB"), "DROP USER ann"), Err(Code::Privilege), "not the owner");
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE T (K INTEGER)").unwrap();
    exec(&s, "INSERT INTO T VALUES (1)").unwrap();
    assert_eq!(exec(&s, "CREATE USER bob PASSWORD 'x'"), Err(Code::TransactionState), "in a transaction");
    s.commit().unwrap();
    for wrong in
        ["CREATE USER bob", "CREATE USER bob PASSWORD x", "CREATE USER bob PASSWORD 1", "CREATE USER PASSWORD 'x'", "DROP USER bob x"]
    {
        assert_eq!(exec(&s, wrong), Err(Code::Syntax), "{wrong}");
    }
    assert_eq!(db.users().unwrap(), ["ANN"], "nothing done");
}

/// Verifies: [EXTENSION] RULES.md D40, D43: the owner's session hands the
/// database on to another user (`Session::set_owner`), told and kept; the
/// session goes on, the owner no more: it manages no user, nor hands the
/// database on again. Refused, nothing changed: by any but the owner
/// (SQLCODE -104), or with none; in a transaction (-917); to one not a
/// user (-103).
#[test]
fn the_owner_hands_the_database_on() {
    let db = Database::new();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone()));
    let ann = db.session("ANN");
    assert_eq!(code(ann.set_owner("ANN")), Some(Code::Privilege), "no owner");
    db.create_user("ANN", b"").unwrap();
    db.create_user("BOB", b"").unwrap();
    assert_eq!(code(db.session("BOB").set_owner("BOB")), Some(Code::Privilege), "not the owner");
    assert_eq!(code(ann.set_owner("CAROL")), Some(Code::SyntaxRule), "not a user");
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE T (K INTEGER)").unwrap();
    exec(&ann, "INSERT INTO T VALUES (1)").unwrap();
    assert_eq!(code(ann.set_owner("BOB")), Some(Code::TransactionState), "in a transaction");
    ann.commit().unwrap();
    assert_eq!(db.owner().unwrap().as_deref(), Some("ANN"), "nothing changed");
    ann.set_owner("BOB").unwrap();
    assert_eq!(db.owner().unwrap().as_deref(), Some("BOB"));
    assert!(kept.taken().iter().any(|(_, e)| e == r#"OwnerChanged { name: "BOB" }"#), "told");
    assert_eq!(exec(&ann, "DROP USER bob"), Err(Code::Privilege), "the owner no more");
    assert_eq!(code(ann.set_owner("ANN")), Some(Code::Privilege));
    assert_eq!(rows(&ann, "SELECT K FROM T"), Ok(vec!["1".to_owned()]), "going on");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4]; [DESIGN] RULES.md D25,
/// D43: ALTER DATABASE OWNER TO hands the database on in SQL, its user an
/// identifier, a regular one upper case; prepared too; kept, so the owner
/// it gives is the owner after a crash, replayed from the log. Refused,
/// nothing changed: by any but the owner (SQLCODE -104); in a transaction
/// (-917); to one not a user (-103), placed at the identifier; and as a
/// Format broken (-102) without OWNER, TO or an identifier.
#[test]
fn the_owner_hands_the_database_on_in_sql() {
    let scratch = Scratch::new("owner-in-sql");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    for user in ["ANN", "BOB", "CAROL"] {
        db.create_user(user, b"").unwrap();
    }
    db.checkpoint().unwrap();
    let ann = db.session("ANN");
    assert_eq!(exec(&db.session("BOB"), "ALTER DATABASE OWNER TO bob"), Err(Code::Privilege), "not the owner");
    assert_eq!(exec(&ann, "ALTER DATABASE OWNER TO dave"), Err(Code::SyntaxRule), "not a user");
    assert_eq!(ann.run("ALTER DATABASE OWNER TO dave").unwrap_err().pos, Some(24), "at the identifier");
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE T (K INTEGER)").unwrap();
    exec(&ann, "INSERT INTO T VALUES (1)").unwrap();
    assert_eq!(exec(&ann, "ALTER DATABASE OWNER TO bob"), Err(Code::TransactionState), "in a transaction");
    ann.commit().unwrap();
    for wrong in ["ALTER DATABASE bob", "ALTER DATABASE OWNER bob", "ALTER DATABASE OWNER TO", "ALTER DATABASE OWNER TO bob x"] {
        assert_eq!(exec(&ann, wrong), Err(Code::Syntax), "{wrong}");
    }
    assert_eq!(db.owner().unwrap().as_deref(), Some("ANN"), "nothing changed");
    exec(&ann, "ALTER DATABASE OWNER TO bob").unwrap();
    assert_eq!(db.owner().unwrap().as_deref(), Some("BOB"));
    let bob = db.session("BOB");
    let handing = bob.prepare("ALTER DATABASE OWNER TO carol").unwrap();
    bob.execute_prepared(&handing, &[]).unwrap();
    assert_eq!(db.owner().unwrap().as_deref(), Some("CAROL"), "prepared");
    drop((ann, bob));
    let crashed = db;
    let recovered = open();
    drop(crashed);
    assert_eq!(recovered.owner().unwrap().as_deref(), Some("CAROL"), "replayed");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.1]; [DESIGN] RULES.md D43:
/// INFORMATION_SCHEMA.USERS shows the owner every user and whether it owns
/// the database; any other user, only itself; an identifier not a user,
/// none. No verifier is a column.
#[test]
fn the_users_table_shows_each_who_it_may() {
    let db = Database::new();
    for user in ["ANN", "BOB", "CAROL"] {
        db.create_user(user, b"secret").unwrap();
    }
    db.set_owner("ANN").unwrap();
    let sql = "SELECT USER_NAME, IS_OWNER FROM INFORMATION_SCHEMA.USERS ORDER BY USER_NAME";
    let shown = |auth: &str| rows(&db.session(auth), sql).unwrap();
    assert_eq!(shown("ANN"), ["ANN,YES", "BOB,NO", "CAROL,NO"]);
    assert_eq!(shown("BOB"), ["BOB,NO"]);
    assert_eq!(shown("DAVE"), Vec::<String>::new());
    assert_eq!(
        rows(&db.session("ANN"), "SELECT COUNT(*) FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'USERS'"),
        Ok(vec!["2".into()])
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4]: an `<authorization
/// identifier>` given alone, as a user names itself to a server, is an
/// `<identifier>`: a regular one in upper case, a delimited one as
/// written; a key word, two identifiers, none, or text not made of
/// tokens is not one.
#[test]
fn a_user_names_itself_by_an_identifier() {
    assert_eq!(authorization_identifier("ann").unwrap(), "ANN");
    assert_eq!(authorization_identifier(" Ann ").unwrap(), "ANN");
    assert_eq!(authorization_identifier(r#""ann b""#).unwrap(), "ann b");
    for wrong in ["USER", "ann bob", "", "a.b"] {
        assert_eq!(authorization_identifier(wrong).err().map(|e| e.code), Some(Code::Syntax), "{wrong}");
    }
    assert_eq!(authorization_identifier("'ann").err().map(|e| e.code), Some(Code::Lexical));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §16.4]; [DESIGN] RULES.md D43: SET SESSION
/// AUTHORIZATION makes the session's identifier the one its value names,
/// trimmed \[GR2\]; a session sets the one it began with, and one begun by
/// the database's owner sets any other, else SQLSTATE 28000 [GR4, GR5].
/// A value not an `<authorization identifier>` is refused, 28000 \[GR3\];
/// one not a character string, SQLCODE -103 \[SR1\]; and it is refused in a
/// transaction, 25000 \[GR1\]. The privileges are then the new identifier's.
#[test]
fn set_session_authorization_sets_whom_a_session_runs_as() {
    let db = Database::new();
    for user in ["ANN", "BOB"] {
        db.create_user(user, b"").unwrap();
    }
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE T (K INTEGER)").unwrap();
    let ann = db.session("ANN");
    exec(&ann, "SET SESSION AUTHORIZATION ' bob '").unwrap();
    assert_eq!(rows(&ann, "SELECT CURRENT_USER, SESSION_USER FROM ANN.T"), Err(Code::Privilege), "as BOB");
    assert_eq!(rows(&ann, "SELECT 1 FROM INFORMATION_SCHEMA.USERS WHERE USER_NAME = CURRENT_USER"), Ok(vec!["1".into()]));
    ann.commit().unwrap();
    exec(&ann, "SET SESSION AUTHORIZATION 'ANN'").unwrap();
    assert_eq!(rows(&ann, "SELECT COUNT(*) FROM T"), Ok(vec!["0".into()]), "ANN's own again");
    assert_eq!(exec(&ann, "SET SESSION AUTHORIZATION 'BOB'"), Err(Code::TransactionState), "in a transaction");
    ann.commit().unwrap();
    let state = |s: &turborel::db::Session, sql: &str| s.execute(sql, &mut Params::new()).err().map(|e| e.sqlstate().as_str().to_owned());
    for wrong in ["'1A'", "'ANN BOB'", "'USER'", "''", "'\"'"] {
        assert_eq!(state(&ann, &format!("SET SESSION AUTHORIZATION {wrong}")).as_deref(), Some("28000"), "{wrong}");
    }
    assert_eq!(exec(&ann, "SET SESSION AUTHORIZATION 1"), Err(Code::SyntaxRule));
    exec(&ann, "SET SESSION AUTHORIZATION '\"some one\"'").unwrap();
    let bob = db.session("BOB");
    assert_eq!(state(&bob, "SET SESSION AUTHORIZATION 'ANN'").as_deref(), Some("28000"), "not the owner's");
    exec(&bob, "SET SESSION AUTHORIZATION 'BOB'").unwrap();
    db.set_owner("BOB").unwrap();
    exec(&bob, "SET SESSION AUTHORIZATION 'ANN'").unwrap();
    assert_eq!(state(&bob, "SET SESSION AUTHORIZATION 'CAROL'"), None, "BOB began it, and owns the database");
}

/// Verifies: [DESIGN] RULES.md D42: CONNECT, SET CONNECTION and DISCONNECT
/// are parsed [ISO/IEC 9075:1992 §15.1-§15.3], but are the SQL-agent's: a
/// session refuses them (SQLCODE -900); a wrong one is a syntax error.
#[test]
fn a_session_refuses_the_connection_statements() {
    let db = Database::new();
    let s = db.session("ANN");
    for sql in [
        "CONNECT TO 'h:1' AS 'C' USER 'ANN'",
        "CONNECT TO DEFAULT",
        "SET CONNECTION 'C'",
        "SET CONNECTION DEFAULT",
        "DISCONNECT ALL",
        "DISCONNECT CURRENT;",
        "DISCONNECT 'C'",
        "DISCONNECT DEFAULT",
    ] {
        assert_eq!(exec(&s, sql), Err(Code::Unsupported), "{sql}");
    }
    for wrong in
        ["CONNECT 'h'", "CONNECT TO", "CONNECT TO 'h' AS", "SET CONNECTION", "DISCONNECT", "DISCONNECT ALL CURRENT", "CONNECT TO 'h' USER"]
    {
        assert_eq!(exec(&s, wrong), Err(Code::Syntax), "{wrong}");
    }
}
