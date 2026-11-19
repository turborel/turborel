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

/// Verifies: [EXTENSION] RULES.md D280, D281: a manifest's settings read
/// back, each a setting of the database's under its extension's name, of
/// its type, range, default and scope, DATABASE if it writes none, NODE of
/// a host plug-in's; and READS SETTINGS names built-in settings.
#[test]
fn a_manifests_settings_read_back() {
    use crate::settings::Kind;
    let m = of("SETTING DEPTH TYPE INTEGER MIN 1 MAX 10 DEFAULT 3 SCOPE SESSION DESCRIPTION 'How deep.';
        SETTING MODE TYPE CHOICE CHOICES (fast, safe); SETTING RATIO TYPE NUMBER MAX 1.5; SETTING ON_OFF TYPE SWITCH;
        SETTING LABEL TYPE TEXT MAX 8 DEFAULT 'a'; SETTING WHO TYPE IDENTIFIER DEFAULT 'X' SCOPE NODE;
        READS SETTINGS ('session.time_zone', 'STATEMENT.TIMEOUT_MS');")
    .unwrap();
    assert_eq!(m.reads_settings, ["SESSION.TIME_ZONE", "STATEMENT.TIMEOUT_MS"]);
    let defined: Vec<crate::settings::Definition> = m.settings.iter().map(|d| d.definition(&m.name, false).unwrap()).collect();
    let depth = &defined[0];
    assert_eq!((depth.name.as_str(), &depth.kind, depth.per_session, depth.node), ("M.DEPTH", &Kind::Integer { min: 1, max: 10 }, true, false));
    assert_eq!((depth.default.to_string(), depth.doc.as_str()), ("3".to_owned(), "How deep."));
    assert_eq!((defined[1].kind.described(), defined[1].default.to_string()), ("ONE OF FAST, SAFE".to_owned(), "FAST".to_owned()));
    assert_eq!((defined[1].per_session, defined[1].node), (false, false));
    assert_eq!((defined[2].default.as_f64(), defined[3].default.to_string()), (Some(0.0), "OFF".to_owned()));
    assert_eq!((&defined[4].kind, defined[5].node), (&Kind::Text { max: 8 }, true));
    assert!(m.settings[0].text.starts_with("SETTING DEPTH TYPE INTEGER"), "{}", m.settings[0].text);
    let host = Manifest::parse("NAME h; PROTOCOL 1; EXECUTABLE 'h'; HOST CLOCK C; SETTING EVERY TYPE INTEGER;").unwrap();
    assert!(host.settings[0].definition(&host.name, true).unwrap().node);
}

/// Verifies: [EXTENSION] RULES.md D280, D281: a setting whose clauses do
/// not fit its type, whose default is not one of its values, declared
/// twice, a host plug-in's not the node's, or a read setting no built-in
/// one, is refused (SQLSTATE 38T06); a type or scope not of the grammar
/// is a syntax error.
#[test]
fn wrong_settings_are_refused() {
    for bad in [
        "SETTING A TYPE INTEGER MIN 5 MAX 1;",
        "SETTING A TYPE NUMBER MIN 2 MAX 1;",
        "SETTING A TYPE INTEGER MIN 0.5;",
        "SETTING A TYPE SWITCH MIN 1;",
        "SETTING A TYPE TEXT MIN 1;",
        "SETTING A TYPE CHOICE;",
        "SETTING A TYPE INTEGER CHOICES (X);",
        "SETTING A TYPE CHOICE CHOICES (X) DEFAULT 'Y';",
        "SETTING A TYPE INTEGER MAX 3 DEFAULT 4;",
        "SETTING A TYPE IDENTIFIER;",
        "SETTING A TYPE TEXT; SETTING a TYPE TEXT;",
        "READS SETTINGS ('NO.SUCH');",
        "READS SETTINGS ('SESSION.TIME_ZONE', 'session.time_zone');",
    ] {
        assert_eq!(state_of(of(bad)), "38T06", "{bad}");
    }
    let host = Manifest::parse("NAME h; PROTOCOL 1; EXECUTABLE 'h'; HOST CLOCK C; SETTING E TYPE INTEGER SCOPE SESSION;");
    assert_eq!(state_of(host), "38T06");
    for bad in ["SETTING A TYPE COLOUR;", "SETTING A TYPE INTEGER SCOPE WORLD;", "SETTING A TYPE INTEGER DEFAULT TRUE;"] {
        assert_eq!(of(bad).err().map(|e| e.code), Some(Code::Syntax), "{bad}");
    }
}
