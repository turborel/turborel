//! Tests of the functions of the mappings of names.

use super::*;

/// CHARACTER VARYING(`length`).
fn varying(length: u32) -> DataType {
    DataType::Varying { length, units: CharUnits::Characters }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.1, §9.3 GR5]
/// RULES.md D106: the result's length is what the mapping of the longest
/// argument can reach: of an XML Name, twice its characters, each a
/// double quote doubled, and the quotes; of an identifier, nine for each
/// character, an escape beyond the Basic Multilingual Plane; at most
/// `MAX_CHARACTER_LENGTH`; of no character string, refused.
#[test]
fn result_types_bound_the_mapping() {
    let (to_identifier, partial, full) = (NameToIdentifier, IdentifierToName(Escape::Partial), IdentifierToName(Escape::Full));
    assert_eq!(to_identifier.return_type(&[varying(10)]).unwrap(), varying(22));
    assert_eq!(partial.return_type(&[DataType::Character { length: 3, units: CharUnits::Octets }]).unwrap(), varying(27));
    assert_eq!(full.return_type(&[varying(MAX_CHARACTER_LENGTH)]).unwrap(), varying(MAX_CHARACTER_LENGTH));
    assert_eq!(partial.return_type(&[DataType::Boolean]).unwrap_err().sqlcode(), -103_i32);
    assert!(to_identifier.return_type(&[]).is_err());
    assert_eq!((partial.name(), full.name()), (IDENTIFIER_TO_XMLNAME, IDENTIFIER_TO_XMLNAME_FULLY));
    assert!(partial.parameter_type(0, &[None]).is_some());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §9.1, §9.3]
/// RULES.md D106: each maps its string, the null value to the null
/// value; a mapping longer than a character varying string holds is
/// SQLSTATE 22001.
#[test]
fn calls_map_names() {
    let call = |f: &dyn ScalarFunction, v: Value| format!("{:?}", f.call(&[v]).unwrap());
    let of = |s: &str| Value::Char(s.into());
    let shown = |s: &str| format!("{:?}", of(s));
    assert_eq!(call(&NameToIdentifier, of("_x0020_a")), shown("\" a\""));
    assert_eq!(call(&IdentifierToName(Escape::Partial), of("a:b c")), shown("a:b_x0020_c"));
    assert_eq!(call(&IdentifierToName(Escape::Full), of("a:b")), shown("a_x003A_b"));
    assert_eq!(call(&NameToIdentifier, Value::Null), format!("{:?}", Value::Null));
    let long = " ".repeat(usize::try_from(MAX_CHARACTER_LENGTH).unwrap());
    let error = IdentifierToName(Escape::Partial).call(&[Value::Char(long)]).unwrap_err();
    assert_eq!(error.sqlstate().as_str(), "22001");
    assert!(NameToIdentifier.call(&[]).is_err());
}
