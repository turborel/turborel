//! The sample frames of every reply, each tail of one, every Status, a
//! Notice and an Error with and without their conditions, and a reply
//! compressed.

use turborel::catalog::{ParameterMode, QualifiedName};
use turborel::params::{DynamicParameter, ParameterType};
use turborel::value::{DataType, Decimal, Spelled, Value};

use super::{Sample, Side, raw, server};
use crate::capability::{
    COMMIT_POSITION, COMPRESSION, CONDITIONS, DECLARED_TYPES, DIAGNOSTICS, GENERATED, MULTI_STATEMENT, NOTICES, PARAMETER_MODES, PING,
    ROW_COUNTS, STATUS,
};
use crate::compress::compress_frames;
use crate::{Called, CancelKey, Column, Completed, Diagnostics, ERROR, Failure, KEY_BYTES, LobReply, Notice, Reply, Timing, WARNING};

/// An INTEGER, its declared type told or not.
fn integer(name: &str, declared: bool) -> Column {
    let column = Column { name: Some(name.into()), data_type: exact(10, 0, Spelled::Integer), not_null: true, declared: None };
    if declared { column.with_declared() } else { column }
}

/// A Prepared of `CALL S.P(?, ?)`, of an IN parameter and an OUT one G,
/// told of its `?`s' modes.
fn called() -> Reply {
    let integer_type = ParameterType::Of(exact(10, 0, Spelled::Integer));
    let of = |mode, ordinal| DynamicParameter { data_type: integer_type.clone(), mode, ordinal: Some(ordinal) };
    let procedure = QualifiedName { auth: "S".into(), ident: "P".into() };
    let called = Called { procedure, parameters: vec![of(ParameterMode::In, 1), of(ParameterMode::Out, 2)] };
    Reply::Prepared { statement: 3, parameters: vec![integer_type.clone()], columns: vec![integer("G", false)], called: Some(called) }
}

/// A Prepared of statement `statement`, of `parameters` and `columns`,
/// told of no `?`'s mode.
fn untold(statement: u32, parameters: Vec<ParameterType>, columns: Vec<Column>) -> Reply {
    Reply::Prepared { statement, parameters, columns, called: None }
}

/// An exact numeric type of `precision` and `scale`, written as `spelled`.
fn exact(precision: u32, scale: u32, spelled: Spelled) -> DataType {
    DataType::Exact { precision, scale, spelled }
}

/// The columns of the declared-type samples: SMALLINT, BIGINT, DECIMAL,
/// REAL, DOUBLE PRECISION and a distinct type, each told.
fn declared_columns() -> Vec<Column> {
    let column =
        |name: &str, data_type: DataType| Column { name: Some(name.into()), data_type, not_null: false, declared: None }.with_declared();
    vec![
        integer("I", true),
        column("S", exact(5, 0, Spelled::Smallint)),
        column("B", exact(18, 0, Spelled::Bigint)),
        column("D", exact(9, 2, Spelled::Decimal)),
        column("R", DataType::Approximate { precision: 24, spelled: Spelled::Real }),
        column("P", DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision }),
        column("M", DataType::distinct("S", "MONEY", exact(9, 2, Spelled::Decimal))),
    ]
}

/// The samples of the login's replies and of a statement's description.
fn described() -> Vec<Sample> {
    let ready = Reply::Ready {
        authorization: "ANN".into(),
        key: CancelKey([0xA5; KEY_BYTES]),
        capabilities: vec![PING.into()],
        server: "turborel 0.0.0 (alpha)".into(),
    };
    let parameters = vec![ParameterType::Varying, ParameterType::Of(exact(10, 0, Spelled::Integer))];
    let unnamed = Column { name: None, data_type: DataType::varying(5), not_null: false, declared: None };
    vec![
        server(
            "challenge",
            &[],
            &Reply::Challenge(b"r=abcdef,s=c2FsdA==,i=4096".to_vec()),
            "Challenge, the bytes r=abcdef,s=c2FsdA==,i=4096",
        ),
        server("ready", &[], &ready, "Ready, authorization ANN, a key of 16 octets A5, capabilities ping, server turborel 0.0.0 (alpha)"),
        server(
            "prepared",
            &[],
            &untold(1, parameters.clone(), vec![integer("K", false), unnamed.clone()]),
            "Prepared statement 1, parameters CHARACTER VARYING flagged varying and exact numeric (10, 0), columns K exact numeric (10, 0) not null and an unnamed CHARACTER VARYING(5) in characters",
        ),
        server(
            "prepared-empty",
            &[],
            &Reply::Prepared { statement: 2, parameters: Vec::new(), columns: Vec::new(), called: None },
            "Prepared statement 2, no parameters, no columns",
        ),
        server(
            "prepared-declared",
            &[DECLARED_TYPES],
            &Reply::Prepared { statement: 1, parameters, columns: vec![integer("K", true)], called: None },
            "Prepared statement 1, two parameters, column K exact numeric (10, 0) not null, tail 19 declared INTEGER, INTEGER",
        ),
        server(
            "description",
            &[],
            &Reply::Description(vec![integer("K", false), unnamed]),
            "Description, columns K exact numeric (10, 0) not null and an unnamed CHARACTER VARYING(5) in characters",
        ),
        server("description-empty", &[], &Reply::Description(Vec::new()), "Description of no columns"),
        server(
            "description-declared",
            &[DECLARED_TYPES],
            &Reply::Description(declared_columns()),
            "Description of I, S, B, D, R, P and M, tail 19 declared INTEGER, SMALLINT, BIGINT, DECIMAL with DECIMAL(9,2), REAL, DOUBLE PRECISION, and USER-DEFINED with S.MONEY",
        ),
    ]
}

/// The samples of the rows and completions of a statement.
fn completions() -> Vec<Sample> {
    let rows = vec![vec![Value::Exact(Decimal::new(1, 0)), Value::Char("a".into())], vec![Value::Null, Value::Char(String::new().into())]];
    vec![
        server("rows", &[], &Reply::Rows(rows), "Rows, two rows, 1 and a, then null and the empty string"),
        server("rows-none", &[], &Reply::Rows(Vec::new()), "Rows of no rows"),
        server("rows-empty-row", &[], &Reply::Rows(vec![Vec::new()]), "Rows, one row of no values"),
        server("suspended", &[], &Reply::Suspended, "Suspended, a header alone, the cursor has more"),
        server("complete", &[], &Reply::Complete(Completed::of(0, "00000")), "Complete, SQLCODE 0, SQLSTATE 00000, active 0, no tail"),
        server(
            "complete-active",
            &[],
            &Reply::Complete(Completed { active: true, ..Completed::of(0, "00000") }),
            "Complete, SQLCODE 0, 00000, active 1",
        ),
        server("complete-no-data", &[], &Reply::Complete(Completed::of(100, "02000")), "Complete, SQLCODE 100, 02000, no data"),
    ]
    .into_iter()
    .chain(tails())
    .collect()
}

/// The samples of a Complete with each tail field.
fn tails() -> Vec<Sample> {
    let every = Completed {
        position: Some(4711),
        more: true,
        rows: Some(3),
        generated: vec!["7".into(), "8".into()],
        time: Some(Timing { total: 1500, queued: 20, locks: 300 }),
        ..Completed::of(0, "00000")
    };
    let warned = Failure {
        sqlcode: 0,
        sqlstate: "01004".into(),
        message: "string data, right truncation".into(),
        rule: None,
        position: None,
        active: true,
        diagnostics: None,
    };
    let conditions = Completed { conditions: vec![warned], active: true, ..Completed::of(0, "01004") };
    vec![
        server(
            "complete-position",
            &[COMMIT_POSITION],
            &Reply::Complete(Completed { position: Some(4711), ..Completed::of(0, "00000") }),
            "Complete, tail 4 position 4711",
        ),
        server(
            "complete-more",
            &[MULTI_STATEMENT],
            &Reply::Complete(Completed { more: true, ..Completed::of(0, "00000") }),
            "Complete, tail 5 more, another statement's replies follow",
        ),
        server(
            "complete-rows",
            &[ROW_COUNTS],
            &Reply::Complete(Completed { rows: Some(1), ..Completed::of(0, "00000") }),
            "Complete, tail 6 rows 1",
        ),
        server(
            "complete-every",
            &[COMMIT_POSITION, MULTI_STATEMENT, ROW_COUNTS, GENERATED],
            &Reply::Complete(every),
            "Complete, tail 4 position 4711, 5 more, 6 rows 3, 16 generated 7 and 8, 18 time total 1500, queued 20, locks 300",
        ),
        server(
            "complete-conditions",
            &[CONDITIONS],
            &Reply::Complete(conditions),
            "Complete, SQLCODE 0, 01004, active 1, tail 17 one condition, a failure as bytes, SQLCODE 0, 01004, string data, right truncation, active 1",
        ),
    ]
}

/// A failure of every field.
fn cited() -> Failure {
    let diagnostics = Diagnostics {
        severity: Some(ERROR.into()),
        detail: Some("key (K)=(1)".into()),
        hint: Some("choose another K".into()),
        context: Some("trigger S.TR".into()),
        schema: Some("S".into()),
        table: Some("T".into()),
        column: Some("K".into()),
        data_type: Some("INTEGER".into()),
        constraint: Some("T_PK".into()),
    };
    Failure {
        sqlcode: -803,
        sqlstate: "23505".into(),
        message: "a unique constraint is broken".into(),
        rule: Some("ISO/IEC 9075-2:2023 §11.7".into()),
        position: Some(12),
        active: true,
        diagnostics: Some(Box::new(diagnostics)),
    }
}

/// The samples of failures, large objects, notices and Pong.
fn conditions() -> Vec<Sample> {
    let plain = Failure { rule: None, position: None, active: false, diagnostics: None, ..cited() };
    let lost = Failure::from(&crate::broken("[DESIGN] a malformed message")).plain();
    let warning = Diagnostics { severity: Some(WARNING.into()), detail: Some("cut to 4".into()), ..Diagnostics::default() };
    let notice = Notice { sqlstate: "01004".into(), message: "string data, right truncation".into(), diagnostics: Diagnostics::default() };
    vec![
        server(
            "error",
            &[],
            &Reply::Error(plain),
            "Error, SQLCODE -803, 23505, a unique constraint is broken, not cited, not placed, active 0, no tail",
        ),
        server(
            "error-cited",
            &[],
            &Reply::Error(Failure { diagnostics: None, ..cited() }),
            "Error, SQLCODE -803, 23505, cited ISO/IEC 9075-2:2023 §11.7, placed at character 12, active 1",
        ),
        server(
            "error-diagnostics",
            &[DIAGNOSTICS],
            &Reply::Error(cited()),
            "Error as error-cited, tail 7 severity ERROR, 8 detail, 9 hint, 10 context, 11 schema S, 12 table T, 13 column K, 14 data type INTEGER, 15 constraint T_PK",
        ),
        server(
            "error-connection",
            &[],
            &Reply::Error(lost),
            "Error, SQLCODE -919, 08006, a malformed message, the connection's, closed after",
        ),
        server("lob-data", &[], &Reply::Lob(LobReply::Data("aé".as_bytes().to_vec())), "Lob data, the octets of aé"),
        server("lob-data-empty", &[], &Reply::Lob(LobReply::Data(Vec::new())), "Lob data of no octets, read past the end"),
        server("lob-locator", &[], &Reply::Lob(LobReply::Locator(4)), "Lob locator 4"),
        server("notice", &[NOTICES], &Reply::Notice(notice.clone()), "Notice, 01004, string data, right truncation, no tail"),
        server(
            "notice-diagnostics",
            &[NOTICES, DIAGNOSTICS],
            &Reply::Notice(Notice { diagnostics: warning, ..notice }),
            "Notice, 01004, tail 7 severity WARNING, 8 detail cut to 4",
        ),
        server("pong", &[PING], &Reply::Pong, "Pong, a header alone"),
    ]
}

/// The samples of every Status the server sends by name, and of one
/// empty, a null's.
fn statuses() -> Vec<Sample> {
    let status = |name: &str, value: &str| Reply::Status { name: name.into(), value: value.into() };
    vec![
        server(
            "status-server-version",
            &[STATUS],
            &status("SERVER_VERSION", "turborel 0.0.0 (alpha)"),
            "Status SERVER_VERSION, turborel 0.0.0 (alpha)",
        ),
        server("status-authorization", &[STATUS], &status("AUTHORIZATION", "ANN"), "Status AUTHORIZATION, ANN"),
        server("status-schema", &[STATUS], &status("SCHEMA", "ANN"), "Status SCHEMA, ANN"),
        server("status-read-only", &[STATUS], &status("READ_ONLY", "NO"), "Status READ_ONLY, NO"),
        server("status-server-read-only", &[STATUS], &status("SERVER_READ_ONLY", "YES"), "Status SERVER_READ_ONLY, YES, as of a replica"),
        server("status-catalog", &[STATUS], &status("CATALOG", "TURBOREL"), "Status CATALOG, TURBOREL, never empty"),
        server("status-owner", &[STATUS], &status("OWNER", "YES"), "Status OWNER, YES"),
        server(
            "status-time-zone",
            &[STATUS],
            &status("SESSION.TIME_ZONE", "+05:30"),
            "Status SESSION.TIME_ZONE, +05:30, a setting the session sets",
        ),
        server("status-null", &[STATUS], &status("STATEMENT.TIMEOUT_MS", ""), "Status STATEMENT.TIMEOUT_MS, the empty string, a null"),
    ]
}

/// A Rows reply compressed, as the server sends one past its threshold.
fn compressed() -> Sample {
    let rows: Vec<Vec<Value>> = (0..64).map(|i| vec![Value::Exact(Decimal::new(i, 0)), Value::Char("abcdefgh".repeat(4).into())]).collect();
    let mut plain = Vec::new();
    Reply::Rows(rows).encode(&mut plain);
    let mut bytes = Vec::new();
    compress_frames(&mut bytes, &plain, 0);
    let length = plain.len().saturating_sub(crate::HEADER);
    let means = format!(
        "Compressed z, holding Rows of 64 rows, 0 to 63 and abcdefgh four times, kind B, length {length}, then its body's zlib stream"
    );
    raw(Side::Server, ("rows-compressed", &[COMPRESSION]), bytes, &means)
}

/// The sample of a Prepared of a CALL, told of its `?`s' modes.
fn told_modes() -> Sample {
    server(
        "prepared-called",
        &[PARAMETER_MODES],
        &called(),
        "Prepared statement 3 of CALL S.P(?, ?): parameter exact numeric (10, 0), column G exact numeric (10, 0) not null, tail 20 called S.P, IN of parameter 1 and OUT of parameter 2, each exact numeric (10, 0)",
    )
}

/// Every reply's samples.
pub(super) fn samples() -> Vec<Sample> {
    let mut all = described();
    all.push(told_modes());
    all.extend(completions());
    all.extend(conditions());
    all.extend(statuses());
    all.push(compressed());
    all
}
