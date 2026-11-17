//! The SQL/MM Full-Text types FULLTEXT and FT_PATTERN and their routines
//! [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.1, §5.3] through SQL: stored,
//! cast, observed and searched; their errors and SQLSTATEs; kept by a
//! checkpoint and recovery. They are built in, so no test registers them.

use turborel::db::{Database, Session};

mod methods;
mod patterns;
mod ranking;
mod recovery;
mod schema;
mod search;
mod thesaurus;
mod tokenizing;

/// Schema A's table DOCS (ID INTEGER, BODY FULLTEXT) of `db`, its rows
/// those of `bodies`, ID from 1, each a FULLTEXT expression or NULL; and a
/// session of A.
pub fn documents(db: &Database, bodies: &[&str]) -> Session {
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE DOCS (ID INTEGER, BODY FULLTEXT)").unwrap();
    let a = db.session("A");
    for (id, body) in bodies.iter().enumerate() {
        a.run(&format!("INSERT INTO DOCS VALUES ({}, {body})", id + 1)).unwrap();
    }
    a
}
