//! Unit tests: each descriptor of a structured type, a typed table, a
//! method, a cast, an ordering and a transform reads back as it was
//! written, and a byte of no kind is refused as corrupt.

use super::*;
use crate::catalog::{ParameterMode, SqlParameter, Stamps, UsageDesc};
use crate::error::Code;
use crate::value::datetime::Now;
use crate::value::{DataType, Spelled};

/// A name in schema A.
fn name(ident: &str) -> QualifiedName {
    QualifiedName { auth: "A".into(), ident: ident.into() }
}

/// A numeric type.
fn numeric() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }
}

/// A grant to PUBLIC and a grantable one to B.
fn grants() -> Vec<UsageDesc> {
    vec![
        UsageDesc { grantor: "A".into(), grantee: None, grantable: false },
        UsageDesc { grantor: "A".into(), grantee: Some("B".into()), grantable: true },
    ]
}

/// A routine a definition makes, named `n`.
fn routine(n: &str) -> RoutineDesc {
    RoutineDesc { name: n.into(), specific: format!("{n}_1"), execute: grants(), stamps: Stamps::at(Now { utc: i64::MIN, zone: 840 }) }
}

/// A method specification of `kind` and `access`, overriding one if
/// `overrides`.
fn method_spec(kind: MethodKind, access: DataAccess, overrides: bool) -> MethodSpecDesc {
    MethodSpecDesc {
        name: "M".into(),
        specific: name("M_1"),
        kind,
        overrides: overrides.then(|| name("M_0")),
        parameters: vec![SqlParameter { name: "N".into(), data_type: numeric(), mode: ParameterMode::In, generic: None }],
        returns: numeric(),
        self_as_result: overrides,
        deterministic: !overrides,
        access,
        null_call: overrides,
        created: 9,
        stamps: Stamps { created: Now { utc: 1, zone: 60 }, last_altered: Now { utc: i64::MAX, zone: -60 } },
    }
}

/// A structured type of `reference`: of a supertype, attributes with and
/// without a default and a collation, methods of each kind and access,
/// reference casts, a constructor and grants, if `full`; of none, else.
fn structured(reference: RefRepresentation, full: bool) -> StructuredDesc {
    let attribute = |n: &str, extra: bool| AttributeDesc {
        name: n.into(),
        data_type: numeric(),
        default: extra.then(|| "1".into()),
        collation: extra.then(|| name("C")),
        defined_by: name("T"),
        observer: routine(n),
        mutator: routine(&format!("{n}_SET")),
    };
    let methods = vec![
        method_spec(MethodKind::Instance, DataAccess::Contains, false),
        method_spec(MethodKind::Static, DataAccess::Reads, true),
        method_spec(MethodKind::Constructor, DataAccess::Modifies, false),
    ];
    StructuredDesc {
        name: name("T"),
        supertype: full.then(|| name("S")),
        supertypes: if full { vec![UdtName { schema: "A".into(), name: "S".into() }] } else { Vec::new() },
        instantiable: full,
        is_final: !full,
        attributes: if full { vec![attribute("X", true), attribute("Y", false)] } else { Vec::new() },
        methods: if full { methods } else { Vec::new() },
        reference,
        ref_casts: full.then(|| ("TO_REF".into(), "TO_TYPE".into())),
        constructor: full.then(|| routine("T")),
        usage: if full { grants() } else { Vec::new() },
        under: if full { grants() } else { Vec::new() },
    }
}

/// `put`'s bytes of `original`, read back by `get`, every byte read.
fn again<T>(original: &T, put: fn(&mut Vec<u8>, &T), get: fn(&mut Reader<'_>) -> Result<T>) -> T {
    let mut out = Vec::new();
    put(&mut out, original);
    let mut r = Reader::new(&out, "descriptor");
    let read = get(&mut r).unwrap();
    r.finish().unwrap();
    read
}

/// The code of reading `written` by `get`, refused.
fn refused<T>(written: &[u8], get: fn(&mut Reader<'_>) -> Result<T>) -> Option<Code> {
    get(&mut Reader::new(written, "descriptor")).err().map(|e| e.code)
}

/// Verifies: [DESIGN] RULES.md D25, D81 [ISO/IEC 9075-2:2023 §4.9.3] a
/// structured type of each reference representation, with and without its
/// optional parts, reads back as written.
#[test]
fn a_structured_type_reads_back_as_written() {
    let references =
        [RefRepresentation::System, RefRepresentation::User(numeric()), RefRepresentation::Derived(vec!["X".into(), "Y".into()])];
    for reference in references {
        for full in [false, true] {
            let original = structured(reference.clone(), full);
            assert_eq!(again(&original, put_structured_type, structured_type), original);
        }
    }
}

/// Verifies: [DESIGN] RULES.md D25, D81 [ISO/IEC 9075-2:2023 §4.17.9] a
/// typed table of each reference generation, with a supertable and
/// without, and a table of no type, read back as written; so does the type
/// and kind of a method, or none.
#[test]
fn typed_tables_and_methods_read_back_as_written() {
    let put = |out: &mut Vec<u8>, t: &Option<TypedDesc>| put_typed(out, t.as_ref());
    for generation in [RefGeneration::System, RefGeneration::User, RefGeneration::Derived] {
        for supertable in [None, Some(name("SUPER"))] {
            let original = Some(TypedDesc { udt: name("T"), self_referencing: "ID".into(), generation, supertable });
            assert_eq!(again(&original, put, typed), original);
        }
    }
    assert_eq!(again(&None, put, typed), None);
    let put_of = |out: &mut Vec<u8>, of: &Option<(QualifiedName, MethodKind)>| put_method_of(out, of.as_ref());
    for of in [None, Some((name("T"), MethodKind::Static)), Some((name("T"), MethodKind::Constructor))] {
        assert_eq!(again(&of, put_of, method_of), of);
    }
}

/// Verifies: [DESIGN] RULES.md D25, D81 [ISO/IEC 9075-2:2023 §11.63,
/// §11.65, §11.67] a cast AS ASSIGNMENT and not, an ordering of each form
/// and category, and transform groups of either function or both, read
/// back as written.
#[test]
fn casts_orderings_and_transforms_read_back_as_written() {
    for assignment in [false, true] {
        let original = CastDesc { source: numeric(), target: DataType::Boolean, function: name("F"), assignment };
        assert_eq!(again(&original, put_cast, cast), original);
    }
    let categories = [
        OrderingCategory::Relative(name("CMP")),
        OrderingCategory::Map(name("MAP")),
        OrderingCategory::State(None),
        OrderingCategory::State(Some(name("EQ"))),
    ];
    for (form, category) in [OrderingForm::Full, OrderingForm::Equals].into_iter().flat_map(|f| categories.clone().map(|c| (f, c))) {
        let original = OrderingDesc { udt: name("T"), form, category };
        assert_eq!(again(&original, put_ordering, ordering), original);
    }
    let groups = vec![
        TransformGroupDesc { name: "G1".into(), to_sql: Some(name("IN")), from_sql: Some(name("OUT")) },
        TransformGroupDesc { name: "G2".into(), to_sql: None, from_sql: Some(name("OUT")) },
        TransformGroupDesc { name: "G3".into(), to_sql: Some(name("IN")), from_sql: None },
    ];
    let original = (name("T"), groups);
    let put = |out: &mut Vec<u8>, (udt, g): &(QualifiedName, Vec<TransformGroupDesc>)| put_transforms(out, (udt, g));
    assert_eq!(again(&original, put, transforms), original);
}

/// Verifies: [DESIGN] RULES.md D25 a byte of no kind, of a method's kind,
/// an SQL-data access indication, a reference representation or
/// generation, a typed table's or a method's flag, or an ordering
/// category, is refused as corrupt.
#[test]
fn a_byte_of_no_kind_is_refused_as_corrupt() {
    let corrupt = Some(Code::Corrupt);
    // A method specification whose kind, then whose access, is no kind.
    let mut bad_kind = Vec::new();
    bad_kind.put_str("M");
    put_name(&mut bad_kind, &name("M_1"));
    bad_kind.put_u8(9);
    assert_eq!(refused(&bad_kind, method), corrupt);
    let mut bad_access = Vec::new();
    bad_access.put_str("M");
    put_name(&mut bad_access, &name("M_1"));
    put_kind(&mut bad_access, MethodKind::Instance);
    put_some_name(&mut bad_access, None);
    put_all(&mut bad_access, std::iter::empty::<&SqlParameter>(), put_parameter);
    put_type(&mut bad_access, &numeric());
    put_flag(&mut bad_access, false);
    put_flag(&mut bad_access, true);
    bad_access.put_u8(9);
    assert_eq!(refused(&bad_access, method), corrupt);
    // A structured type whose reference representation is none.
    let mut bad_reference = Vec::new();
    put_name(&mut bad_reference, &name("T"));
    put_some_name(&mut bad_reference, None);
    put_all(&mut bad_reference, std::iter::empty::<&UdtName>(), |_, _| {});
    put_flag(&mut bad_reference, true);
    put_flag(&mut bad_reference, false);
    put_all(&mut bad_reference, std::iter::empty::<&AttributeDesc>(), put_attribute);
    put_all(&mut bad_reference, std::iter::empty::<&MethodSpecDesc>(), put_method);
    bad_reference.put_u8(9);
    assert_eq!(refused(&bad_reference, structured_type), corrupt);
    // A typed table's flag, and its generation.
    assert_eq!(refused(&[9], typed), corrupt);
    let mut bad_generation = vec![SOME];
    put_name(&mut bad_generation, &name("T"));
    bad_generation.put_str("ID");
    bad_generation.put_u8(9);
    assert_eq!(refused(&bad_generation, typed), corrupt);
    // A method's flag, and an ordering's category.
    assert_eq!(refused(&[9], method_of), corrupt);
    let mut bad_category = Vec::new();
    put_name(&mut bad_category, &name("T"));
    put_flag(&mut bad_category, true);
    bad_category.put_u8(9);
    assert_eq!(refused(&bad_category, ordering), corrupt);
}
