//! Unit tests of a node's settings (RULES.md R-TEST-1): the members
//! named, and a node refused before it opens.

use turborel::db::Database;
use turborel::error::Code;
use turborel::value::{Decimal, Value};
use turborel_replication::{ReplicaId, Transport};

use super::net::{host_and_port, members_of};
use super::prepare;
use crate::settings::{self, MEMBERS, REPLICA, REPLICATION_CA, REPLICATION_TLS, TLS_CERTIFICATE, TLS_KEY};

/// `s` as a value: a whole number if it is one, else text.
fn value(s: &str) -> Value {
    s.parse::<i128>().map_or_else(|_| Value::Char(s.to_owned()), |n| Value::Exact(Decimal::new(n, 0)))
}

/// A database with the replica's and the server's settings declared, and
/// `given` set.
fn configured(given: &[(&str, &str)]) -> Database {
    let db = Database::new();
    settings::declare_replication(&db).unwrap();
    settings::declare(&db).unwrap();
    for (name, text) in given {
        db.set_setting(name, &value(text)).unwrap();
    }
    db
}

/// Verifies: [EXTENSION] RULES.md D57: REPLICATION.MEMBERS names each
/// member by its number and host:port, by commas, spaces around them
/// ignored, in the order of the numbers; an entry without `=`, a number
/// 0, a number named twice, or an address without a port is refused
/// (SQLCODE -910); none named is no member.
#[test]
fn members_are_named_by_number_and_address() {
    let members = members_of(" 2 = db2.example.com:7423, 1=localhost:7423 ,3=[::1]:7423").unwrap();
    let listed: Vec<(u32, &str)> = members.iter().map(|(id, address)| (id.0, address.as_str())).collect();
    assert_eq!(listed, [(1, "localhost:7423"), (2, "db2.example.com:7423"), (3, "[::1]:7423")]);
    assert_eq!(host_and_port("[::1]:7423").unwrap(), ("[::1]".to_owned(), 7423));
    assert!(members_of("").unwrap().is_empty());
    for wrong in ["1", "0=localhost:1", "1=localhost", "1=localhost:x", "1=a:1,1=b:1", "x=a:1"] {
        assert_eq!(members_of(wrong).unwrap_err().code, Code::Api, "{wrong}");
    }
    assert_eq!(members.get(&ReplicaId(1)).map(String::as_str), Some("localhost:7423"));
}

/// Verifies: [EXTENSION] RULES.md D57: a database whose
/// REPLICATION.REPLICA is 0 is not replicated, and prepared as nothing;
/// one whose number is no member, whose members are malformed, or whose
/// TLS is ON without its files named, is refused (SQLCODE -910) before
/// it opens; with TLS OFF, it is prepared, its log storage the group's.
#[test]
fn a_node_is_refused_before_it_opens_if_its_settings_are_wrong() {
    assert!(prepare(&configured(&[]), std::path::Path::new("/db"), false).unwrap().is_none(), "not replicated");
    let members = "1=localhost:7431,2=localhost:7432";
    let refused = |given: &[(&str, &str)]| prepare(&configured(given), std::path::Path::new("/db"), false).err().map(|e| e.code);
    assert_eq!(refused(&[(REPLICA, "3"), (MEMBERS, members), (REPLICATION_TLS, "OFF")]), Some(Code::Api), "no member");
    assert_eq!(refused(&[(REPLICA, "1"), (MEMBERS, "1=localhost"), (REPLICATION_TLS, "OFF")]), Some(Code::Api), "malformed");
    assert_eq!(refused(&[(REPLICA, "1"), (MEMBERS, members)]), Some(Code::Api), "TLS ON, no files");
    let files = [
        (REPLICA, "1"),
        (MEMBERS, members),
        (TLS_CERTIFICATE, "/no/such/chain.pem"),
        (TLS_KEY, "/no/such/key.pem"),
        (REPLICATION_CA, "/no/such/ca.pem"),
    ];
    assert_eq!(refused(&files), Some(Code::Io), "TLS ON, files missing");
    let db = configured(&[(REPLICA, "2"), (MEMBERS, members), (REPLICATION_TLS, "OFF")]);
    let prepared = prepare(&db, std::path::Path::new("/db"), true).unwrap().unwrap();
    assert_eq!(prepared.transport.listen(), "localhost:7432");
    assert_eq!(Transport::members(&*prepared.transport), [ReplicaId(1), ReplicaId(2)]);
}
