//! The sample frames of every request, each tail of one, and a request
//! compressed.

use turborel::storage::lanes::{Lane, Lanes};
use turborel::value::Value;
use turborel::value::lob::LobKind;

use super::{Sample, Side, client, raw};
use crate::capability::{
    CHANGE_USER, COLUMNS, COMPRESSION, CONNECTION_ATTRIBUTES, COPY, CURSORS, DECLARED_TYPES, DESCRIBE, MULTI_STATEMENT, PING,
    QUERY_ATTRIBUTES, RESET, STATUS,
};
use crate::compress::compress_frames;
use crate::{CancelKey, Described, KEY_BYTES, LobRequest, MAX_ATTRIBUTE_NAME, MAX_ATTRIBUTE_VALUE, MAX_ATTRIBUTES, Request, VERSION};

/// A Hello as ANN by `trust`, asking `capabilities`, of `attributes`.
fn hello(capabilities: &[&str], attributes: Vec<(String, String)>) -> Request {
    Request::Hello {
        version: VERSION,
        user: "ANN".into(),
        mechanism: "trust".into(),
        initial: Vec::new(),
        capabilities: capabilities.iter().map(|c| (*c).to_owned()).collect(),
        attributes,
    }
}

/// A Query of `sql`, its transaction ended with it if `ends`.
fn query(sql: &str, ends: bool) -> Request {
    Request::Query { sql: sql.into(), ends, attributes: Vec::new(), multi: false }
}

/// The samples of Hello, Answer and Cancel, before a session.
fn opening() -> Vec<Sample> {
    let longest = vec![("a".repeat(MAX_ATTRIBUTE_NAME), "v".repeat(MAX_ATTRIBUTE_VALUE))];
    let most = (0..MAX_ATTRIBUTES).map(|i| (format!("a{i}"), String::new())).collect();
    let scram = Request::Hello {
        version: VERSION,
        user: "ANN".into(),
        mechanism: "scram-sha-256".into(),
        initial: b"n,,n=,r=abc".to_vec(),
        capabilities: Vec::new(),
        attributes: Vec::new(),
    };
    vec![
        client(
            "hello-ping",
            &[],
            &hello(&[PING], Vec::new()),
            "Hello, version 1, user ANN, mechanism trust, no first bytes, asking ping, no tail",
        ),
        client("hello-none", &[], &hello(&[], Vec::new()), "Hello asking no capability, the protocol's first form"),
        client(
            "hello-every",
            &[],
            &hello(&[STATUS, DESCRIBE, CURSORS, COMPRESSION, DECLARED_TYPES], vec![("application".into(), "app".into())]),
            "Hello asking status, describe, cursors, compression and declared-types, tail 1 one attribute application=app",
        ),
        client(
            "hello-attribute-longest",
            &[CONNECTION_ATTRIBUTES],
            &hello(&[], longest),
            "Hello, tail 1 one attribute of the longest name (64 bytes) and value (1024 bytes)",
        ),
        client(
            "hello-attributes-most",
            &[CONNECTION_ATTRIBUTES],
            &hello(&[], most),
            "Hello, tail 1 the most attributes (32), a0 to a31, each value the empty string",
        ),
        client("hello-scram", &[], &scram, "Hello, mechanism scram-sha-256, first bytes the client-first message n,,n=,r=abc"),
        client("answer", &[], &Request::Answer(b"c=biws,r=abc,p=xyz".to_vec()), "Answer, the bytes c=biws,r=abc,p=xyz"),
        client("answer-empty", &[], &Request::Answer(Vec::new()), "Answer of no bytes, as SCRAM's last step"),
        client("cancel", &[], &Request::Cancel(CancelKey([0xA5; KEY_BYTES])), "Cancel, a key of 16 octets A5"),
    ]
}

/// The samples of the statements' requests.
fn statements() -> Vec<Sample> {
    vec![
        client("query", &[], &query("VALUES 1", true), "Query VALUES 1, ends 1, no tail"),
        client("query-empty", &[], &query("", false), "Query of the empty string, ends 0"),
        client(
            "query-attributes",
            &[QUERY_ATTRIBUTES],
            &Request::Query {
                sql: "VALUES QUERY_ATTRIBUTE('trace')".into(),
                ends: false,
                attributes: vec![("trace".into(), "t1".into())],
                multi: false,
            },
            "Query, ends 0, tail 1 one attribute trace=t1",
        ),
        client(
            "query-multi",
            &[MULTI_STATEMENT],
            &Request::Query { sql: "VALUES 1; VALUES 2".into(), ends: true, attributes: Vec::new(), multi: true },
            "Query of two statements, ends 1, tail 2 multi",
        ),
        client(
            "prepare",
            &[],
            &Request::Prepare { statement: 1, sql: "SELECT K FROM T WHERE K > ?".into() },
            "Prepare statement 1, SELECT K FROM T WHERE K > ?",
        ),
    ]
    .into_iter()
    .chain(executions())
    .collect()
}

/// The samples of Execute, Fetch, Close and Deallocate.
fn executions() -> Vec<Sample> {
    let values = vec![Value::Null, Value::Char(String::new().into()), Value::Bool(true)];
    vec![
        client(
            "execute",
            &[],
            &Request::Execute { statement: 1, cursor: 0, values, rows: 2, ends: false, attributes: Vec::new() },
            "Execute statement 1, values null, the empty string and true, rows 2, ends 0, cursor 0, no tail",
        ),
        client(
            "execute-cursor",
            &[CURSORS, QUERY_ATTRIBUTES],
            &Request::Execute {
                statement: 1,
                cursor: 7,
                values: Vec::new(),
                rows: 0,
                ends: true,
                attributes: vec![("a".into(), "b".into())],
            },
            "Execute statement 1, no values, rows 0 for all, ends 1, tail 1 attribute a=b, tail 3 cursor 7",
        ),
        client("fetch", &[], &Request::Fetch { statement: 1, cursor: 0, rows: 100 }, "Fetch statement 1, rows 100, no tail"),
        client(
            "fetch-cursor",
            &[CURSORS],
            &Request::Fetch { statement: 1, cursor: 7, rows: u32::MAX },
            "Fetch statement 1, rows 4294967295, tail 3 cursor 7",
        ),
        client("close", &[], &Request::Close { statement: 1, cursor: 0 }, "Close statement 1, cursor 0, no tail"),
        client("close-cursor", &[CURSORS], &Request::Close { statement: 1, cursor: 7 }, "Close statement 1, tail 3 cursor 7"),
        client("deallocate", &[], &Request::Deallocate { statement: u32::MAX }, "Deallocate statement 4294967295"),
        client("terminate", &[], &Request::Terminate, "Terminate, a header alone"),
    ]
}

/// The samples of Lob, Describe and the requests of a session.
fn session() -> Vec<Sample> {
    let read = LobRequest::Read { locator: 3, from: 0, count: 65_536, characters: true };
    vec![
        client("lob-read", &[], &Request::Lob(read), "Lob read, locator 3, from 0, count 65536, in characters"),
        client("lob-create", &[], &Request::Lob(LobRequest::Create(LobKind::Binary)), "Lob create, binary"),
        client(
            "lob-append",
            &[],
            &Request::Lob(LobRequest::Append { locator: 4, bytes: vec![0xC3] }),
            "Lob append to locator 4, the octet C3, half a character",
        ),
        client("lob-free", &[], &Request::Lob(LobRequest::Free(4)), "Lob free, locator 4"),
        client("lob-hold", &[], &Request::Lob(LobRequest::Hold(3)), "Lob hold, locator 3"),
        client(
            "describe-statement",
            &[DESCRIBE],
            &Request::Describe(Described::Statement(1)),
            "Describe a statement, statement 1, cursor 0",
        ),
        client(
            "describe-cursor",
            &[DESCRIBE],
            &Request::Describe(Described::Cursor { statement: 1, cursor: 7 }),
            "Describe a cursor, statement 1, cursor 7",
        ),
        client("ping", &[PING], &Request::Ping, "Ping, a header alone"),
        client("reset", &[RESET], &Request::Reset, "Reset, a header alone"),
    ]
    .into_iter()
    .chain(renewals())
    .collect()
}

/// The samples of ChangeUser and of a copy.
fn renewals() -> Vec<Sample> {
    vec![
        client(
            "change-user",
            &[CHANGE_USER],
            &Request::ChangeUser { user: "BOB".into(), mechanism: "password".into(), initial: b"secret".to_vec() },
            "ChangeUser to BOB, mechanism password, first bytes secret",
        ),
        client(
            "copy-in",
            &[COPY],
            &Request::CopyIn { table: "S.T".into(), columns: vec!["ID".into(), "NAME".into()], ends: true },
            "CopyIn table S.T, columns ID and NAME, ends 1",
        ),
        client(
            "copy-in-every-column",
            &[COPY],
            &Request::CopyIn { table: "T".into(), columns: Vec::new(), ends: false },
            "CopyIn table T, no columns for every column, ends 0",
        ),
        client(
            "copy-data",
            &[COPY],
            &Request::CopyData(vec![vec![Value::Char("1".into()), Value::Null], vec![Value::Char("2".into()), Value::Char("b".into())]]),
            "CopyData of two rows, 1 and null, 2 and b",
        ),
        client(
            "copy-columns",
            &[COPY, COLUMNS],
            &Request::CopyColumns(Lanes {
                rows: 2,
                columns: vec![
                    Lane { nulls: vec![0b10], ends: vec![1, 1], bytes: b"1".to_vec() },
                    Lane { nulls: vec![0], ends: vec![1, 1], bytes: b"b".to_vec() },
                ],
            }),
            "CopyColumns of two rows by column, 1 and null, b and the empty string",
        ),
        client("copy-end", &[COPY], &Request::CopyEnd { abandon: false }, "CopyEnd, abandon 0"),
        client("copy-end-abandon", &[COPY], &Request::CopyEnd { abandon: true }, "CopyEnd, abandon 1"),
    ]
}

/// A Query compressed, as a client granted `compression` sends one.
fn compressed() -> Sample {
    let mut plain = Vec::new();
    query(&format!("SELECT '{}' FROM T", "x".repeat(2000)), true).encode(&mut plain);
    let mut bytes = Vec::new();
    compress_frames(&mut bytes, &plain, 0);
    let length = plain.len().saturating_sub(crate::HEADER);
    let means = format!(
        "Compressed z, holding Query SELECT 'x...x' FROM T of 2000 x, ends 1, kind Q, length {length}, then its body's zlib stream"
    );
    raw(Side::Client, ("query-compressed", &[COMPRESSION]), bytes, &means)
}

/// Every request's samples.
pub(super) fn samples() -> Vec<Sample> {
    let mut all = opening();
    all.extend(statements());
    all.extend(session());
    all.push(compressed());
    all
}
