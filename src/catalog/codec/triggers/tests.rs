//! Unit tests: a trigger descriptor reads back as `put_trigger` wrote it.

use super::*;
use crate::catalog::{Horizon, QualifiedName};
use crate::error::Code;
use crate::value::datetime::Now;

/// A name in schema A.
fn name(ident: &str) -> QualifiedName {
    QualifiedName { auth: "A".into(), ident: ident.into() }
}

/// A trigger of these parts, with transition names, a horizon and a
/// triggered action column set.
fn desc(time: ActionTime, level: TriggerLevel, event: TriggerEvent, names: TransitionNames) -> TriggerDesc {
    TriggerDesc {
        name: name("TR"),
        table: name("T"),
        time,
        level,
        event,
        names,
        text: "CREATE TRIGGER TR AFTER INSERT ON T FOR EACH ROW DELETE FROM U".into(),
        schema: "A".into(),
        reads: vec![name("T"), name("U")],
        horizon: Horizon {
            widths: vec![(name("T"), 2), (name("U"), 3)],
            routines: Some(vec![name("F_1")]),
            xml_schemas: vec![name("X")],
            xquery_modules: vec![name("M")],
            static_typing: true,
        },
        columns: vec![(name("T"), "K".into()), (name("U"), "V".into())],
        sequences: vec![name("Q")],
        created: 7,
        created_at: Now { utc: -1, zone: -720 },
    }
}

/// Every transition variable and table name.
fn all_names() -> TransitionNames {
    TransitionNames { old_row: Some("O".into()), new_row: Some("N".into()), old_table: Some("OT".into()), new_table: Some("NT".into()) }
}

/// `original` in bytes.
fn bytes(original: &TriggerDesc) -> Vec<u8> {
    let mut out = Vec::new();
    put_trigger(&mut out, original);
    out
}

/// `original` written and read back, every byte read.
fn again(original: &TriggerDesc) -> TriggerDesc {
    let written = bytes(original);
    let mut r = Reader::new(&written, "trigger");
    let read = trigger(&mut r).unwrap();
    r.finish().unwrap();
    read
}

/// Verifies: [DESIGN] RULES.md D25, [ISO/IEC 9075-2:2023 §4.46.1] each
/// trigger action time, level and event, UPDATE with a trigger column list
/// and without, reads back as written, with its horizon and its triggered
/// action column set.
#[test]
fn each_kind_of_trigger_reads_back_as_written() {
    let events =
        [TriggerEvent::Insert, TriggerEvent::Delete, TriggerEvent::Update(Vec::new()), TriggerEvent::Update(vec!["K".into(), "V".into()])];
    for time in [ActionTime::Before, ActionTime::After, ActionTime::InsteadOf] {
        for level in [TriggerLevel::Row, TriggerLevel::Statement] {
            for event in &events {
                let original = desc(time, level, event.clone(), all_names());
                assert_eq!(format!("{:?}", again(&original)), format!("{original:?}"));
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D25, [ISO/IEC 9075-2:2023 §4.46.1] the
/// transition names read back as written, all of them and none.
#[test]
fn transition_names_read_back_all_and_none() {
    for names in [all_names(), TransitionNames::default()] {
        let mut original = desc(ActionTime::After, TriggerLevel::Statement, TriggerEvent::Delete, names);
        original.horizon = Horizon::default();
        original.columns.clear();
        assert_eq!(format!("{:?}", again(&original)), format!("{original:?}"));
    }
}

/// Verifies: [DESIGN] RULES.md D25, a trigger cut short at any byte, or of
/// a tag not known, is refused as corrupt (SQLCODE -916), never a panic.
#[test]
fn a_trigger_cut_short_or_mistagged_is_refused() {
    let written = bytes(&desc(ActionTime::Before, TriggerLevel::Row, TriggerEvent::Update(vec!["K".into()]), all_names()));
    for end in 0..written.len() {
        let err = trigger(&mut Reader::new(&written[..end], "trigger")).unwrap_err();
        assert_eq!(err.code, Code::Corrupt, "cut at {end}");
    }
    // The action time's tag follows the two names, "A" and "TR", "A" and
    // "T", each string after its length's 4 bytes.
    let at = 22_usize;
    let mut mistagged = written;
    mistagged[at] = 9;
    let err = trigger(&mut Reader::new(&mistagged, "trigger")).unwrap_err();
    assert_eq!(err.code, Code::Corrupt);
}
