//! Tests of the thirteen types [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §15.1 to §15.13] and the supported units [§15.1.1 Description 16].

use super::*;

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15] the thirteen
/// types, named as the standard spells them and found by name in any case.
#[test]
fn every_type_is_named() {
    assert_eq!(ALL.len(), KINDS);
    for k in ALL {
        assert_eq!(LrKind::named(k.type_name()), Some(k));
        assert_eq!(LrKind::named(&k.sql_name().to_ascii_lowercase()), Some(k));
        assert_eq!(LrKind::of_code(k.code()), Some(k));
    }
    assert_eq!(LrKind::named("ST_Point"), None);
    assert_eq!(LrKind::of_code(13), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.2.1, §15.3.1,
/// §15.4.1, §15.5.1] ST_LinearElement is the one type not instantiable,
/// and the supertype of the three element types alone.
#[test]
fn linear_element_is_the_supertype() {
    for k in ALL {
        assert_eq!(k.is_instantiable(), k != LrKind::LinearElement);
        let element = matches!(k, LrKind::LrFeature | LrKind::LrCurve | LrKind::LrDirectedEdge);
        assert_eq!(k.supertype() == Some(LrKind::LinearElement), element);
        assert_eq!(k.is_subtype_of(LrKind::LinearElement), element || k == LrKind::LinearElement);
        assert!(k.is_subtype_of(k));
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.1 Description
/// 16] the supported units are the seeded LINEAR ones, named in any case.
#[test]
fn units_are_the_linear_seed() {
    let all = units();
    assert_eq!(metres_in(METRE, &all), Some(1.0_f64));
    assert_eq!(metres_in("KILOMETRE", &all), Some(1000.0_f64));
    assert_eq!(metres_in("degree", &all), None);
    assert!(supported("foot", &all, "§test").is_ok());
    assert_eq!(state(supported("furlong", &all, "§test")), SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.3.1, §15.4.1,
/// §15.5.1] a value's kind is its most specific type.
#[test]
fn values_know_their_type() {
    let c = LinearElement::Curve(road());
    assert_eq!(LrValue::LinearElement(c.clone()).kind(), LrKind::LrCurve);
    assert_eq!(c.as_curve().map(|x| x.base.id), Some(1_i32));
    assert!(c.as_feature().is_none());
    assert_eq!(LrValue::LrMeasure(m(1.0_f64)).kind(), LrKind::LrMeasure);
    assert_eq!(LrValue::Lrm(lrm(1, ABSOLUTE)).kind(), LrKind::Lrm);
}

/// Verifies: [DESIGN] a catalog finds its LRMs, elements and features.
#[test]
fn catalog_finds_by_id() {
    let all = units();
    let lrms = [lrm(1, ABSOLUTE)];
    let feature = LrFeature::new(parts(7, 1), Some("F".to_owned())).unwrap();
    let elements = [LinearElement::Curve(road()), LinearElement::Feature(feature)];
    let cat = Catalog { units: &all, lrms: &lrms, elements: &elements, metres_per_unit: 1.0_f64 };
    assert_eq!(cat.lrm(1).map(|l| l.id), Some(1_i32));
    assert!(cat.lrm(2).is_none());
    assert_eq!(cat.element(7).map(LinearElement::kind), Some(LrKind::LrFeature));
    assert_eq!(cat.feature("F").map(|f| f.base.id), Some(7_i32));
    assert!(cat.feature("G").is_none());
    assert!(Catalog::of_units(&all).element(1).is_none());
}

/// Verifies: [DESIGN] nesting past MAX_DEPTH is refused (RULES.md R-CF-6).
#[test]
fn depth_is_bounded() {
    assert_eq!(deeper(0).unwrap(), 1);
    assert_eq!(deeper(MAX_DEPTH).unwrap_err().code, Code::Unsupported);
}
