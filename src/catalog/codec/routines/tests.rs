//! Unit tests: a routine descriptor reads back as `put_routine` wrote it.

use super::*;
use crate::catalog::{Horizon, QualifiedName, UsageDesc};
use crate::error::Code;
use crate::value::{DataType, Spelled};

/// A name in schema A.
fn name(ident: &str) -> QualifiedName {
    QualifiedName { auth: "A".into(), ident: ident.into() }
}

/// A numeric type.
fn numeric() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }
}

/// A routine of these parts, with a parameter of each mode, a horizon,
/// routines it invokes and EXECUTE grants to PUBLIC and to a user.
fn desc(kind: RoutineKind, access: DataAccess, returns: Option<DataType>) -> SqlRoutineDesc {
    let parameter = |n: &str, mode| SqlParameter { name: n.into(), data_type: numeric(), mode };
    SqlRoutineDesc {
        name: name("F"),
        specific: name("F_1"),
        kind,
        parameters: vec![parameter("X", ParameterMode::In), parameter("Y", ParameterMode::Out), parameter("Z", ParameterMode::InOut)],
        returns,
        deterministic: true,
        access,
        null_call: true,
        text: "CREATE FUNCTION F (X NUMERIC) RETURNS NUMERIC RETURN (SELECT K FROM T)".into(),
        body: "RETURN (SELECT K FROM T)".into(),
        schema: "A".into(),
        module: Some("SHOP".into()),
        reads: vec![name("T"), name("V")],
        horizon: Horizon { widths: vec![(name("T"), 2)], routines: Some(vec![name("G_1")]) },
        sequences: vec![name("Q"), name("R")],
        execute: vec![
            UsageDesc { grantor: "A".into(), grantee: None, grantable: false },
            UsageDesc { grantor: "A".into(), grantee: Some("B".into()), grantable: true },
        ],
        created: 7,
    }
}

/// `original` in bytes.
fn bytes(original: &SqlRoutineDesc) -> Vec<u8> {
    let mut out = Vec::new();
    put_routine(&mut out, original);
    out
}

/// `original` written and read back, every byte read.
fn again(original: &SqlRoutineDesc) -> SqlRoutineDesc {
    let written = bytes(original);
    let mut r = Reader::new(&written, "routine");
    let read = routine(&mut r).unwrap();
    r.finish().unwrap();
    read
}

/// The code of reading `written`, refused.
fn refused(written: &[u8]) -> Option<Code> {
    routine(&mut Reader::new(written, "routine")).err().map(|e| e.code)
}

/// Verifies: [DESIGN] RULES.md D25, [ISO/IEC 9075-2:2023 §4.35.5] each
/// kind of routine and SQL-data access indication, with a result type and
/// without, reads back as written, with its parameters of every mode, its
/// horizon, the routines it invokes and its EXECUTE privileges.
#[test]
fn each_kind_of_routine_reads_back_as_written() {
    for kind in [RoutineKind::Function, RoutineKind::Procedure] {
        for access in [DataAccess::Contains, DataAccess::Reads, DataAccess::Modifies] {
            for returns in [Some(numeric()), Some(DataType::Boolean), None] {
                let original = desc(kind, access, returns);
                assert_eq!(format!("{:?}", again(&original)), format!("{original:?}"));
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D25, [ISO/IEC 9075-2:2023 §4.35.5] a
/// routine of no parameters, reads, horizon, invoked routines or grants,
/// neither deterministic nor a null-call function, reads back as written.
#[test]
fn an_empty_routine_reads_back_as_written() {
    let mut original = desc(RoutineKind::Procedure, DataAccess::Modifies, None);
    original.parameters.clear();
    original.reads.clear();
    original.horizon = Horizon::default();
    original.execute.clear();
    (original.deterministic, original.null_call) = (false, false);
    assert_eq!(format!("{:?}", again(&original)), format!("{original:?}"));
}

/// Verifies: [DESIGN] RULES.md D25, R-ERR-1: a routine whose kind, access
/// indication or parameter mode tag names none of them is SQLCODE -916,
/// never misread, and so is one cut short.
#[test]
fn unknown_tags_are_corrupt() {
    let mut one = desc(RoutineKind::Function, DataAccess::Reads, Some(numeric()));
    one.parameters.truncate(1);
    let written = bytes(&one);
    let mut prefix = Vec::new();
    put_name(&mut prefix, &one.name);
    put_name(&mut prefix, &one.specific);
    let kind_at = prefix.len();
    let mode_at = {
        let mut p = prefix.clone();
        p.put_u8(FUNCTION);
        p.put_u32(1);
        p.put_str("X");
        put_type(&mut p, &numeric());
        p.len()
    };
    let access_at = {
        let mut p = Vec::new();
        put_type(&mut p, &numeric());
        mode_at + 1 + 1 + p.len() + 1
    };
    for (at, tag, what) in [(kind_at, FUNCTION, "a kind"), (mode_at, IN, "a mode"), (access_at, READS, "an access indication")] {
        let mut bad = written.clone();
        assert_eq!(bad[at], tag, "{what} is where it is looked for");
        bad[at] = 9;
        assert_eq!(refused(&bad), Some(Code::Corrupt), "{what}");
    }
    assert_eq!(refused(&written[..written.len() - 1]), Some(Code::Corrupt), "cut short");
}
