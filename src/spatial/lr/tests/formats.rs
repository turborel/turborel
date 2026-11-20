//! Tests of the stored form, the well-known text [ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §15.14] and the GML representation [§15.1.12 and its
//! peers] of every type: each value reads back to itself, and what is not
//! the representation is refused.

use super::*;

/// A distance expression with every attribute: referents with feature
/// ids, a lateral offset from a geometry and a vertical one of no
/// distance from a description.
fn full_distance() -> DistanceExp {
    let names = ReferentNames {
        from_feature: Some("F1".to_owned()),
        from_name: Some("km0".to_owned()),
        towards_feature: None,
        towards_name: Some("w \"x\"".to_owned()),
    };
    let lateral = LatOffsetExp::with_geometry(m(1.5_f64), point(1.0_f64, 2.0_f64));
    let vertical = VerOffsetExp { distance: None, geometry: None, description: Some("deck".to_owned()) };
    DistanceExp::new(Some(m(5.0_f64)), names, Some(lateral), Some(vertical)).unwrap()
}

/// A distance expression of vector offsets: a 2D, a 3D and an empty one.
fn vector_distance() -> DistanceExp {
    let vectors = VectorOffsetExp::new(vec![
        Vector::xy(1.0_f64, -2.0_f64),
        Vector::xyz(0.5_f64, 0.0_f64, 3.0_f64),
        Vector { x: None, y: None, z: None },
    ])
    .unwrap();
    DistanceExp::new_vector(Some(LrMeasure::new(-7.25_f64)), ReferentNames::default(), Some(vectors)).unwrap()
}

/// A feature with start values, no type, and two referents: one at a
/// position, one at a location.
fn full_feature() -> LinearElement {
    let all = units();
    let cat = Catalog::of_units(&all);
    let km0 = Referent::new(Some("km0".to_owned()), Some("post".to_owned()), Some(point(20.0_f64, 0.0_f64)), None, &cat).unwrap();
    let where_ = PositionExp::of_ids(1, 1, DistanceExp::along(m(5.0_f64)));
    let loc = Referent::new(Some("loc".to_owned()), Some("post".to_owned()), None, Some(where_), &cat).unwrap();
    let p = ElementParts { element_type: None, start_values: Some(vec![StartValue::new(1, m(0.0_f64))]), ..parts(9, 3) };
    LinearElement::Feature(LrFeature::new_with_referents(p, Some("F1".to_owned()), Some(vec![km0, loc])).unwrap())
}

/// One value or more of every type.
pub(super) fn samples() -> Vec<LrValue> {
    let all = units();
    let survey = LrMeasure::new_with_units(Some(1.0_f64), Some("US survey foot".to_owned()), &all).unwrap();
    let edge = EdgeParts { topology_type: Some('L'), topology_name: Some("net".to_owned()), edge_id: Some(12_i32) };
    let edge = LinearElement::Edge(LrDirectedEdge::new(ElementParts { start_values: Some(Vec::new()), ..parts(4, 1) }, edge).unwrap());
    let bare = LinearElement::Feature(LrFeature::new_with_referents(parts(8, 1), Some("F2".to_owned()), None).unwrap());
    let empty = LinearElement::Feature(LrFeature::new(parts(8, 1), Some("F3".to_owned())).unwrap());
    let full = PositionExp::of_element_and_lrm(LinearElement::Curve(road()), lrm_offsets(5, ABSOLUTE, LEFT), full_distance());
    vec![
        LrValue::Lrm(lrm(1, ABSOLUTE)),
        LrValue::Lrm(lrm_offsets(5, ABSOLUTE, LEFT)),
        LrValue::LinearElement(full_feature()),
        LrValue::LinearElement(bare),
        LrValue::LinearElement(empty),
        LrValue::LinearElement(LinearElement::Curve(road())),
        LrValue::LinearElement(edge),
        LrValue::PositionExp(PositionExp::of_ids(1, 2, DistanceExp::along(m(3.0_f64)))),
        LrValue::PositionExp(PositionExp::of_id_and_lrm(1, lrm(2, RELATIVE), vector_distance())),
        LrValue::PositionExp(PositionExp::of_element_and_id(full_feature(), 3, DistanceExp::along(m(3.0_f64)))),
        LrValue::PositionExp(full),
        LrValue::LrMeasure(m(2.5_f64)),
        LrValue::LrMeasure(LrMeasure::new(3.0_f64)),
        LrValue::LrMeasure(survey),
        LrValue::StartValue(StartValue::new(2, m(-1.0_f64))),
        LrValue::DistanceExp(full_distance()),
        LrValue::DistanceExp(vector_distance()),
        LrValue::Referent(
            Referent::new(Some("r".to_owned()), Some("t".to_owned()), Some(point(1.0_f64, 1.0_f64)), None, &Catalog::of_units(&all))
                .unwrap(),
        ),
        LrValue::LatOffsetExp(LatOffsetExp::with_description(m(2.0_f64), "kerb".to_owned())),
        LrValue::VerOffsetExp(VerOffsetExp::with_geometry(m(2.0_f64), line(&[0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64], Dims::Xy))),
        LrValue::VectorOffsetExp(VectorOffsetExp::new(vec![Vector::xy(1.0_f64, 2.0_f64)]).unwrap()),
    ]
}

/// Verifies: [DESIGN] every value reads back from its stored form.
#[test]
fn stored_form_round_trips() {
    for v in samples() {
        let bytes = v.encode().unwrap();
        assert_eq!(LrValue::decode(&bytes).unwrap(), v);
    }
}

/// Verifies: [DESIGN] bytes the encoder did not write are corruption
/// (SQLCODE -916), never a panic.
#[test]
fn bad_bytes_are_corrupt() {
    let good = LrValue::PositionExp(PositionExp::of_ids(1, 2, full_distance())).encode().unwrap();
    for cut in 0..good.len() {
        assert_eq!(LrValue::decode(&good[..cut]).unwrap_err().code, Code::Corrupt);
    }
    let mut trailing = good.clone();
    trailing.push(0);
    assert_eq!(LrValue::decode(&trailing).unwrap_err().code, Code::Corrupt);
    let mut version = good.clone();
    version[0] = 2;
    assert_eq!(LrValue::decode(&version).unwrap_err().code, Code::Corrupt);
    let mut kind = good;
    kind[1] = LrKind::LinearElement.code();
    assert_eq!(LrValue::decode(&kind).unwrap_err().code, Code::Corrupt);
    assert_eq!(LrValue::decode(&[1, 200]).unwrap_err().code, Code::Corrupt);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.14] every value
/// reads back from its well-known text, as its own type and as
/// ST_LinearElement for an element.
#[test]
fn text_round_trips() {
    let all = units();
    let cat = Catalog::of_units(&all);
    for v in samples() {
        let text = as_text(&v).unwrap();
        let back = from_text(&text, v.kind(), &cat).unwrap_or_else(|e| panic!("{text}: {e:?}"));
        assert_eq!(back, v, "{text}");
        assert_eq!(from_text_or_gml(&text, v.kind(), &cat, &NoSrs).unwrap(), v);
    }
    let element = as_text(&LrValue::LinearElement(LinearElement::Curve(road()))).unwrap();
    assert_eq!(from_text(&element, LrKind::LinearElement, &cat).unwrap().kind(), LrKind::LrCurve);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.14.1 to §15.14.4]
/// the text of the BNF reads, in any case of its key words.
#[test]
fn standard_text_reads() {
    let all = units();
    let cat = Catalog::of_units(&all);
    let p = from_text("posexp (linearelement leid 1), (LRM LRMID 2), (DISEXP MEASURE 2.5E1 metre)", LrKind::PositionExp, &cat).unwrap();
    assert_eq!(p, LrValue::PositionExp(PositionExp::of_ids(1, 2, DistanceExp::along(m(25.0_f64)))));
    let text = "LRM LRMID 3, LRMNAME \"r\", LRMTYPE \"relative\", LRMUOM \"metre\", LRMOFFSETUOM \"metre\", POSVEROFFDIR \"down\"";
    let LrValue::Lrm(l) = from_text(text, LrKind::Lrm, &cat).unwrap() else { panic!("not an LRM") };
    assert_eq!((l.pos_lat_offset_dir(), l.pos_ver_offset_dir()), (Some(RIGHT), Some(DOWN)));
    let edge = "LINEARELEMENT LRDIRECTEDEDGE LEID 1, DEFAULT LRMID 1, DEFAULT MEASURE 10, LETYPE \"t\", E \"topo\" 5";
    assert_eq!(from_text(edge, LrKind::LrDirectedEdge, &cat).unwrap().kind(), LrKind::LrDirectedEdge);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.6.9 Description
/// 2a] text not producible in the BNF, or of another type, raises 2FF22;
/// a constructor's exception is the text's.
#[test]
fn bad_text_is_refused() {
    let all = units();
    let cat = Catalog::of_units(&all);
    let wkt = SqlState::SPATIAL_INVALID_WKT;
    for text in [
        "",
        "POINT (1 2)",
        "LRM LRMID 1",
        "LINEARELEMENT LEID 1",
        "DISEXP MEASURE",
        "DISEXP MEASURE 1 metre,",
        "DISEXP MEASURE 1 metre )",
        "MEASURE 1 \"metre",
        "LRM LRMID x",
        "DISEXP MEASURE 1, VECTOROFFSETS (VECTOR (1 2), VECTOR (1 2), VECTOR (1 2), VECTOR (1 2))",
    ] {
        assert_eq!(state(from_text(text, LrKind::DistanceExp, &cat)), wkt, "{text}");
    }
    assert_eq!(state(from_text("MEASURE 1 metre", LrKind::DistanceExp, &cat)), wkt);
    assert_eq!(state(from_text("MEASURE 1 cubit", LrKind::LrMeasure, &cat)), SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED);
    let towards = "DISEXP MEASURE 1 TOWARDS \"b\"";
    assert_eq!(state(from_text(towards, LrKind::DistanceExp, &cat)), SqlState::SPATIAL_TOWARDS_REFERENT_REQUIRES_FROM);
    let mixed = "VECTOROFFSETS (VECTOR Z (1 2))";
    assert_eq!(state(from_text(mixed, LrKind::VectorOffsetExp, &cat)), SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS);
    let flat = "VECTOROFFSETS (VECTOR (1 2 3))";
    assert_eq!(state(from_text(flat, LrKind::VectorOffsetExp, &cat)), SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS);
    let not_point = "(NAME \"a\", TYPE \"b\", POSITION LINESTRING (0 0, 1 1))";
    assert_eq!(state(from_text(not_point, LrKind::Referent, &cat)), wkt);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.12 and its
/// peers] every value reads back from its GML.
#[test]
fn gml_round_trips() {
    let all = units();
    let cat = Catalog::of_units(&all);
    for v in samples() {
        let text = as_gml(&v, &NoSrs).unwrap();
        let back = from_gml(&text, v.kind(), &cat, &NoSrs).unwrap_or_else(|e| panic!("{text}: {e:?}"));
        assert_eq!(back, v, "{text}");
        assert_eq!(from_text_or_gml(&format!("  {text}"), v.kind(), &cat, &NoSrs).unwrap(), v);
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.12 and its
/// peers] a document that is not the representation, or of another type,
/// raises 2FF24; a constructor's exception is the document's.
#[test]
fn bad_gml_is_refused() {
    let all = units();
    let cat = Catalog::of_units(&all);
    let gml = SqlState::SPATIAL_INVALID_GML;
    for text in [
        "<LRM>",
        "<Point/>",
        "<LRMeasure>x</LRMeasure>",
        "<LRM><lrmID>1</lrmID></LRM>",
        "<LRCurve><linearElementID>1</linearElementID></LRCurve>",
        "<VectorOffsetExpression><Vector><x>1</x></Vector></VectorOffsetExpression>",
    ] {
        assert_eq!(state(from_gml(text, LrKind::Lrm, &cat, &NoSrs)), gml, "{text}");
    }
    assert_eq!(state(from_gml("<LRMeasure>1</LRMeasure>", LrKind::Lrm, &cat, &NoSrs)), gml);
    let unit = "<LRMeasure uom=\"cubit\">1</LRMeasure>";
    assert_eq!(state(from_gml(unit, LrKind::LrMeasure, &cat, &NoSrs)), SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED);
    let towards = "<DistanceExpression><distanceAlong>1</distanceAlong><towardsReferentName>b</towardsReferentName></DistanceExpression>";
    assert_eq!(state(from_gml(towards, LrKind::DistanceExp, &cat, &NoSrs)), SqlState::SPATIAL_TOWARDS_REFERENT_REQUIRES_FROM);
}

/// Verifies: [DESIGN] ST_MaxLRAsText bounds the text read (RULES.md D85).
#[test]
fn text_is_bounded() {
    let all = units();
    let cat = Catalog::of_units(&all);
    let long = " ".repeat(MAX_TEXT.saturating_add(1));
    assert_eq!(from_text(&long, LrKind::Lrm, &cat).unwrap_err().code, Code::Unsupported);
    assert_eq!(from_gml(&long, LrKind::Lrm, &cat, &NoSrs).unwrap_err().code, Code::Unsupported);
}

/// Verifies: [DESIGN] the stored form's own checks: a presence octet
/// other than 0 or 1, an element inside a position expression whose type
/// code names a type that is no linear element, and more than three
/// vectors are corruption (SQLCODE -916).
#[test]
fn stored_form_checks_its_codes() {
    let position = LrValue::PositionExp(PositionExp::of_element_and_id(full_feature(), 3, DistanceExp::along(m(3.0_f64))));
    let good = position.encode().unwrap();
    assert_eq!(good[6], 1, "the element's presence octet");
    assert_eq!(good[7], LrKind::LrFeature.code(), "the element's type code");
    let mut presence = good.clone();
    presence[6] = 2;
    assert_eq!(LrValue::decode(&presence).unwrap_err().code, Code::Corrupt);
    for kind in [LrKind::Lrm, LrKind::LinearElement, LrKind::VectorOffsetExp] {
        let mut not_element = good.clone();
        not_element[7] = kind.code();
        assert_eq!(LrValue::decode(&not_element).unwrap_err().code, Code::Corrupt, "{kind:?}");
    }
    let vectors = LrValue::VectorOffsetExp(VectorOffsetExp::new(vec![Vector::xy(1.0_f64, 2.0_f64)]).unwrap());
    let mut four = vectors.encode().unwrap();
    assert_eq!(four[2..6], 1_u32.to_le_bytes());
    four[2..6].copy_from_slice(&4_u32.to_le_bytes());
    four.extend(four[6..].to_vec().repeat(3));
    assert_eq!(LrValue::decode(&four).unwrap_err().code, Code::Corrupt);
}
