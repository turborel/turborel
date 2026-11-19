//! Unit tests of manifests, format 1: what they declare, read back, and
//! the rules of the format refused (RULES.md R-TEST-1, R-TEST-2).

use super::*;
use crate::error::Code;

/// A manifest of `rest` after its required items.
fn of(rest: &str) -> Result<Manifest> {
    Manifest::parse(&format!("NAME m; VERSION '1.2.3'; PROTOCOL 1; EXECUTABLE 'bin/m';\n{rest}"))
}

/// The SQLSTATE of `text`'s refusal.
fn state_of(made: Result<Manifest>) -> String {
    made.err().and_then(|e| e.state).map(|s| s.as_str().to_owned()).unwrap_or_default()
}

/// Verifies: [EXTENSION] RULES.md D168: a manifest's items read back:
/// its name folded as an identifier, its version, executables, packages'
/// items, and each object with its clauses; its kinds sorted once each.
#[test]
fn a_manifest_reads_back_its_declarations() {
    let m = of("EXECUTABLE FOR 'windows' 'bin\\m.exe'; REQUIRES base '^1.0'; TURBOREL '>=0.1'; UPGRADE FROM '<1.2.0' SCRIPT 'up.sql';
        FUNCTION F(INTEGER, VARCHAR(10)) RETURNS INTEGER DETERMINISTIC BATCH 10;
        TABLE FUNCTION T(INTEGER) RETURNS TABLE (A INTEGER, B DOUBLE PRECISION) READS SQL DATA;
        PROCEDURE P() MODIFIES SQL DATA;
        METRIC D FORMATS (FLOAT32) PROPERTIES (SYMMETRIC);
        EMBEDDING MODEL E DIMENSIONS 3 ELEMENT FLOAT16 NOT DETERMINISTIC;
        AUTHENTICATOR 'TOKEN'; TARGET 'store'; PARTITION STRATEGY S;")
    .unwrap();
    assert_eq!((m.name.as_str(), m.version.as_str()), ("M", "1.2.3"));
    assert_eq!(m.executable("linux").unwrap(), PathBuf::from("bin/m"));
    assert_eq!(m.executable("windows").unwrap(), PathBuf::from("bin\\m.exe"));
    assert_eq!(m.requires, [("BASE".to_owned(), "^1.0".to_owned())]);
    assert_eq!((m.turborel.as_deref(), m.upgrades.len()), (Some(">=0.1"), 1));
    assert_eq!(m.kinds(), [1, 3, 4, 5, 6, 7, 8, 9]);
    let f = &m.objects[0];
    assert_eq!((f.parameters.len(), f.deterministic, f.batch), (2, true, Some(10)));
    assert!(f.text.starts_with("FUNCTION F(") && f.text.ends_with("BATCH 10"), "{}", f.text);
    assert_eq!((m.objects[1].columns.len(), m.objects[1].reads, m.objects[2].modifies), (2, true, true));
    assert!(m.objects[3].has_property(SYMMETRIC) && !m.objects[3].has_property(TRIANGLE));
    assert_eq!((m.objects[4].dimensions, m.objects[4].element.as_deref()), (Some(3), Some("FLOAT16")));
    assert_eq!((m.objects[5].name.as_str(), m.objects[6].name.as_str()), ("TOKEN", "store"));
}

/// Verifies: [EXTENSION] RULES.md D168: a manifest breaking format 1 is
/// refused with SQLSTATE 38T06: an item written twice, PROTOCOL not 1 or
/// not written, no NAME or EXECUTABLE, a path outside the directory, an
/// object declared twice, a BATCH of 0, an aggregate of two arguments or
/// named as a standard set function, an unknown property or element
/// format, and no dimensions; a broken grammar is a syntax error.
#[test]
fn a_manifest_breaking_format_1_is_refused() {
    for bad in [
        "NAME n;",
        "VERSION '2';",
        "PROTOCOL 1;",
        "UPGRADE FROM '1' SCRIPT '../up.sql';",
        "FUNCTION F() RETURNS INTEGER; FUNCTION F() RETURNS INTEGER;",
        "FUNCTION F() RETURNS INTEGER BATCH 0;",
        "AGGREGATE A(INTEGER, INTEGER) RETURNS INTEGER;",
        "AGGREGATE \"SUM\"(INTEGER) RETURNS INTEGER;",
        "METRIC D PROPERTIES (ROUND);",
        "EMBEDDING MODEL E DIMENSIONS 0;",
        "EMBEDDING MODEL E DIMENSIONS 2 ELEMENT CHARS;",
        "TURBOREL '1'; TURBOREL '2';",
        "EXECUTABLE 'other';",
    ] {
        assert_eq!(state_of(of(bad)), "38T06", "{bad}");
    }
    for bad in [
        "NAME m; VERSION '1'; PROTOCOL 2; EXECUTABLE 'm';",
        "NAME m; VERSION '1'; EXECUTABLE 'm';",
        "VERSION '1'; PROTOCOL 1; EXECUTABLE 'm';",
        "NAME m; PROTOCOL 1;",
        "NAME m; PROTOCOL 1; EXECUTABLE '/bin/m';",
    ] {
        assert_eq!(state_of(Manifest::parse(bad)), "38T06", "{bad}");
    }
    assert_eq!(of("FUNCTION F RETURNS INTEGER;").err().map(|e| e.code), Some(Code::Syntax));
    assert_eq!(of("WIDGET W;").err().map(|e| e.code), Some(Code::Syntax));
}

/// Verifies: [EXTENSION] RULES.md D168: a path inside the directory is
/// relative and never climbs out of it.
#[test]
fn paths_stay_inside_the_directory() {
    for good in ["m", "bin/m", "./bin/m"] {
        assert!(inside(good, "EXECUTABLE").is_ok(), "{good}");
    }
    for bad in ["", "/bin/m", "../m", "bin/../../m"] {
        assert!(inside(bad, "EXECUTABLE").is_err(), "{bad}");
    }
}

/// Verifies: [EXTENSION] RULES.md D275, D276: an optimizer rule is kind
/// 37, its PHASE BOUND or REWRITTEN and its PASSES a count above 0, each
/// unless written none; a phase of another name, or no passes, is
/// refused.
#[test]
fn an_optimizer_rule_names_its_phase_and_passes() {
    let m = of("OPTIMIZER RULE R PHASE BOUND PASSES 3; OPTIMIZER RULE S;").unwrap();
    assert_eq!(m.kinds(), [37]);
    let (r, s) = (&m.objects[0], &m.objects[1]);
    assert_eq!((r.kind, r.phase.as_deref(), r.passes), (ObjectKind::Rule, Some(BOUND), Some(3)));
    assert_eq!((s.phase.as_deref(), s.passes), (None, None));
    assert_eq!(state_of(of("OPTIMIZER RULE R PHASE LATE;")), "38T06");
    assert_eq!(state_of(of("OPTIMIZER RULE R PASSES 0;")), "38T06");
    assert_eq!(of("OPTIMIZER RULE R(INTEGER);").err().map(|e| e.code), Some(Code::Syntax));
}
