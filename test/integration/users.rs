//! Users and the database's owner (RULES.md D43): kept with the
//! definitions, managed by the host program's API and, in SQL, by the
//! owner alone; their verifiers made by a credentials plug-in.

use turborel::db::{Credentials, Database, OpenOptions, Opened};
use turborel::error::{Code, Result};
use turborel::lexer::authorization_identifier;

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
/// owns the database. The sink hears of each change,
/// never of a verifier.
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
    db.drop_user("ANN").unwrap();
    assert!(!db.users().unwrap().contains(&"ANN".to_owned()));
    let events = kept.taken().into_iter().map(|(_, e)| e).collect::<Vec<_>>().join("\n");
    assert!(events.contains(r#"UserChanged { name: "ANN", dropped: false }"#), "{events}");
    assert!(events.contains(r#"UserChanged { name: "ANN", dropped: true }"#), "{events}");
    assert!(events.contains(r#"OwnerChanged { name: "ANN" }"#), "the first user, made the owner: {events}");
    assert!(events.contains(r#"OwnerChanged { name: "bob" }"#), "{events}");
    assert!(!events.contains("two"), "no verifier is told");
}

/// Verifies: [DESIGN] RULES.md D25, D43: users and the owner are kept with
/// the definitions: those made before a checkpoint come back from it, those
/// made after from the log; after a crash as after a close.
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
/// folded to upper case, a delimited one as written; WITH is optional.
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
    let pad = |name: &str| format!("{name:<128}");
    assert_eq!(shown("ANN"), [format!("{},YES", pad("ANN")), format!("{},NO ", pad("BOB")), format!("{},NO ", pad("CAROL"))]);
    assert_eq!(shown("BOB"), [format!("{},NO ", pad("BOB"))]);
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
