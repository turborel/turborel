//! \[EXTENSION\] Where a model comes from and what depends on it (the
//! vector design's §12.1, §16): CREATE, ALTER and DROP EXTERNAL MODEL,
//! the compatibility alias; CREATE EMBEDDING MODEL FROM BLOB, of a literal
//! or a parameter, through reopen; a view that calls a model, dropped with
//! it by CASCADE alone; and DROP ROLE taking the grants of EXECUTE on a
//! model it held or made, and what they held up.

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{Binary, DataType, Value};

use crate::common::{Scratch, after_crash, exec, rows};
use crate::embedding_models::counted::{database, prepared, registered, registered_as};

/// Verifies: [EXTENSION] vector design §12.1 compatibility aliases: CREATE
/// EXTERNAL MODEL is an embedding model of the plug-in REMOTE, its
/// LOCATION given the plug-in as ENDPOINT, its MODEL_TYPE EMBEDDINGS and
/// no other; ALTER and DROP EXTERNAL MODEL alter and drop it.
#[test]
fn an_external_model_is_an_embedding_model_of_remote() {
    let (db, _) = database();
    let remote = registered_as(&db, "REMOTE");
    let s = db.session("U");
    s.run(
        "CREATE EXTERNAL MODEL E WITH (LOCATION = 'https://example.com/v1/embeddings', API_FORMAT = 'OpenAI',
           MODEL_TYPE = EMBEDDINGS, MODEL = 'small', CREDENTIAL = 'key')",
    )
    .unwrap();
    let options = remote.options.lock().unwrap().clone();
    for given in ["ENDPOINT=https://example.com/v1/embeddings", "API_FORMAT=OpenAI", "MODEL=small", "CREDENTIAL=key"] {
        assert!(options.contains(&given.to_owned()), "{given} in {options:?}");
    }
    assert!(!options.iter().any(|o| o.starts_with("MODEL_TYPE") || o.starts_with("LOCATION")), "{options:?}");
    let shown = "SELECT MODEL_NAME, MODEL_TYPE, DIMENSIONS FROM INFORMATION_SCHEMA.EMBEDDING_MODELS";
    assert_eq!(rows(&s, shown).unwrap(), ["E,REMOTE,4"]);
    assert_eq!(rows(&s, "VALUES (EMBED('E', 'four'))").unwrap(), ["[4,1,0,0]"]);
    s.run("ALTER EXTERNAL MODEL E WITH (LOCATION = 'https://example.com/v2', VERSION = 2)").unwrap();
    assert!(remote.options.lock().unwrap().contains(&"ENDPOINT=https://example.com/v2".to_owned()));
    assert_eq!(rows(&s, "VALUES (EMBED_VERSION(E))").unwrap(), ["U.E:2"]);
    assert_eq!(exec(&s, "CREATE EXTERNAL MODEL F WITH (LOCATION = 'x', MODEL_TYPE = GENERATION)"), Err(Code::Syntax));
    s.run("CREATE EXTERNAL MODEL C WITH (TYPE = 'COUNTED', MODEL_TYPE = 'embeddings')").unwrap();
    assert_eq!(rows(&s, "SELECT MODEL_TYPE FROM INFORMATION_SCHEMA.EMBEDDING_MODELS WHERE MODEL_NAME = 'C'").unwrap(), ["COUNTED"]);
    s.run("DROP EXTERNAL MODEL E").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.EMBEDDING_MODELS").unwrap(), ["1"]);
}

/// Verifies: [EXTENSION] vector design §12.1 FROM BLOB: a model made FROM
/// BLOB gives its plug-in the octets, of a binary string literal or a
/// parameter; a plug-in that reads no model's file refuses them, as a
/// value not a binary string is refused; the octets survive a crash, the
/// plug-in given them again as the database opens.
#[test]
fn a_model_from_blob_is_given_its_octets() {
    let scratch = Scratch::new("embedding-blob");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        prepared(&db);
        let asked = registered(&db);
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        (db, asked)
    };
    let (db, asked) = open();
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    let s = db.session("U");
    s.run("CREATE EMBEDDING MODEL L FROM BLOB X'00FF07' WITH (TYPE = 'COUNTED')").unwrap();
    let octets = Value::Binary(Binary::new(vec![1, 2]));
    let mut params = Params::new().declare("B", DataType::VarBinary { length: 16 }, octets);
    s.execute("CREATE EMBEDDING MODEL P FROM BLOB B WITH (TYPE = 'COUNTED', VERSION = 5)", &mut params).unwrap();
    let options = asked.options.lock().unwrap().clone();
    assert!(options.contains(&"BLOB=[0, 255, 7]".to_owned()) && options.contains(&"BLOB=[1, 2]".to_owned()), "{options:?}");
    assert_eq!(exec(&s, "CREATE EMBEDDING MODEL H FROM BLOB X'00' WITH (TYPE = 'HASH_DENSE')"), Err(Code::SyntaxRule));
    assert_eq!(exec(&s, "CREATE EMBEDDING MODEL H FROM BLOB 'text' WITH (TYPE = 'COUNTED')"), Err(Code::SyntaxRule));
    let mut text = Params::new().declare("B", DataType::varying(4), Value::Char("text".into()));
    assert_eq!(s.execute("CREATE EMBEDDING MODEL H FROM BLOB B WITH (TYPE = 'COUNTED')", &mut text).unwrap_err().code, Code::SyntaxRule);
    s.commit().unwrap();
    drop((s, db));
    let (db, asked) = open();
    let s = db.session("U");
    assert_eq!(rows(&s, "VALUES (EMBED_VERSION(P), EMBED('L', 'ab'))").unwrap(), ["U.P:5,[2,1,0,0]"]);
    let options = asked.options.lock().unwrap().clone();
    assert!(options.contains(&"BLOB=[0, 255, 7]".to_owned()) && options.contains(&"BLOB=[1, 2]".to_owned()), "{options:?}");
}

/// Verifies: [EXTENSION] vector design §12.1: a view that calls a model,
/// by a literal or a bare name, depends on it: DROP EMBEDDING MODEL
/// RESTRICT is refused, and CASCADE drops it and the views that read it;
/// a view that names a plug-in alone does not.
#[test]
fn a_view_that_calls_a_model_depends_on_it() {
    let (db, _) = database();
    let s = db.session("U");
    s.run("CREATE EMBEDDING MODEL M WITH (TYPE = 'COUNTED')").unwrap();
    s.run("CREATE TABLE T (X VARCHAR(20))").unwrap();
    s.run("INSERT INTO T VALUES ('abc')").unwrap();
    s.run("CREATE VIEW E (V) AS SELECT EMBED('M', X) FROM T").unwrap();
    s.run("CREATE VIEW F AS SELECT V FROM E").unwrap();
    s.run("CREATE VIEW G (V) AS SELECT EMBED_VERSION(M) FROM T").unwrap();
    s.run("CREATE VIEW H (V) AS SELECT EMBED('COUNTED', X) FROM T").unwrap();
    assert_eq!(rows(&s, "SELECT V FROM F").unwrap(), ["[3,1,0,0]"]);
    assert_eq!(exec(&s, "DROP EMBEDDING MODEL M RESTRICT"), Err(Code::SyntaxRule));
    s.run("DROP EMBEDDING MODEL M CASCADE").unwrap();
    let views = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = 'U' ORDER BY 1";
    assert_eq!(rows(&s, views).unwrap(), ["H"]);
    assert_eq!(rows(&s, "SELECT V FROM H").unwrap(), ["[3,1,0,0]"]);
}

/// Verifies: [EXTENSION] vector design §16; [ISO/IEC 9075-2:2023 §12.6
/// GR1, §12.7 GR8] DROP ROLE takes the grants of EXECUTE on a model made
/// to the role: a user who held EXECUTE through the role alone holds it
/// no more, and a user who holds it of their own still does.
#[test]
fn drop_role_takes_its_grants_on_models() {
    let (db, _) = database();
    for user in ["U", "V", "W"] {
        db.create_user(user, b"x").unwrap();
    }
    db.allow_role_creator("U", true).unwrap();
    let u = db.session("U");
    u.run("CREATE EMBEDDING MODEL M WITH (TYPE = 'COUNTED')").unwrap();
    u.run("CREATE ROLE R").unwrap();
    u.run("GRANT EXECUTE ON EMBEDDING MODEL M TO R WITH GRANT OPTION").unwrap();
    u.run("GRANT EXECUTE ON EMBEDDING MODEL M TO W").unwrap();
    u.run("GRANT R TO V").unwrap();
    u.run("GRANT R TO W").unwrap();
    u.commit().unwrap();
    let call = "VALUES (EMBED('U.M', 'abc'))";
    let (v, w) = (db.session("V"), db.session("W"));
    v.run("SET ROLE 'R'").unwrap();
    assert_eq!(rows(&v, call).unwrap(), ["[3,1,0,0]"], "through R");
    v.commit().unwrap();
    let privileges = "SELECT GRANTOR, GRANTEE, IS_GRANTABLE FROM INFORMATION_SCHEMA.EMBEDDING_MODEL_PRIVILEGES ORDER BY 1, 2";
    assert_eq!(rows(&u, privileges).unwrap(), ["U,R,YES", "U,W,NO", "_SYSTEM,U,YES"]);
    u.run("DROP ROLE R CASCADE").unwrap();
    u.commit().unwrap();
    assert_eq!(rows(&u, privileges).unwrap(), ["U,W,NO", "_SYSTEM,U,YES"]);
    let v = db.session("V");
    assert_eq!(rows(&v, call), Err(Code::Privilege));
    assert_eq!(rows(&w, call).unwrap(), ["[3,1,0,0]"], "of its own");
}
