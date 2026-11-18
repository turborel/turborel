//! Tests of the constructors, observers and mutators [ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §15.1.2 to §15.13.3]: their values, defaults, null
//! rules and exceptions.

use super::*;

/// The LRM parts of `lrm(1, ABSOLUTE)`, to change one.
fn lrm_parts() -> LrmParts {
    LrmParts {
        id: Some(1),
        name: Some("km".to_owned()),
        lrm_type: Some(ABSOLUTE.to_owned()),
        units: Some(METRE.to_owned()),
        constraints: None,
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.2 Description
/// 4a, 4e] ST_LRM refuses a null argument and an unsupported unit.
#[test]
fn lrm_constructor_checks() {
    let all = units();
    let l = Lrm::new(lrm_parts(), &all).unwrap();
    assert_eq!((l.lrm_id(), l.lrm_name(), l.lrm_type(), l.unit_of_measure()), (1_i32, "km", ABSOLUTE, METRE));
    assert!(l.constraints().is_none() && l.offset_meas_unit().is_none() && !l.allows_offsets());
    assert_eq!(state(Lrm::new(LrmParts { id: None, ..lrm_parts() }, &all)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(Lrm::new(LrmParts { name: None, ..lrm_parts() }, &all)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(Lrm::new(LrmParts { lrm_type: None, ..lrm_parts() }, &all)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(Lrm::new(LrmParts { units: None, ..lrm_parts() }, &all)), SqlState::SPATIAL_NULL_ARGUMENT);
    let bad = LrmParts { units: Some("league".to_owned()), ..lrm_parts() };
    assert_eq!(state(Lrm::new(bad, &all)), SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.2 Description
/// 6f, 6g vii, viii] an offset unit is checked, and the directions default
/// to 'right' and 'up' only of an LRM allowing offsets.
#[test]
fn lrm_offset_defaults() {
    let all = units();
    let with = OffsetParts { units: Some(METRE.to_owned()), lateral: None, vertical: None };
    let l = Lrm::new_with_offsets(lrm_parts(), with, &all).unwrap();
    assert_eq!((l.pos_lat_offset_dir(), l.pos_ver_offset_dir()), (Some(RIGHT), Some(UP)));
    let without = OffsetParts { units: None, lateral: Some(LEFT.to_owned()), vertical: Some(DOWN.to_owned()) };
    let plain = Lrm::new_with_offsets(lrm_parts(), without, &all).unwrap();
    assert_eq!((plain.pos_lat_offset_dir(), plain.pos_ver_offset_dir()), (None, None));
    let bad = OffsetParts { units: Some("cubit".to_owned()), lateral: None, vertical: None };
    assert_eq!(state(Lrm::new_with_offsets(lrm_parts(), bad, &all)), SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.3 to §15.1.7
/// Description 4] the LRM mutators: null argument first, then a null SELF
/// gives the null value.
#[test]
fn lrm_mutators() {
    let all = units();
    let l = lrm(1, ABSOLUTE);
    assert_eq!(Lrm::with_lrm_id(Some(&l), Some(9_i32)).unwrap().unwrap().lrm_id(), 9_i32);
    assert_eq!(state(Lrm::with_lrm_id(None, None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(Lrm::with_lrm_id(None, Some(9_i32)).unwrap(), None);
    assert_eq!(Lrm::with_lrm_name(Some(&l), Some("n".to_owned())).unwrap().unwrap().lrm_name(), "n");
    assert_eq!(state(Lrm::with_lrm_name(Some(&l), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(Lrm::with_lrm_type(Some(&l), Some(RELATIVE.to_owned())).unwrap().unwrap().lrm_type(), RELATIVE);
    assert_eq!(state(Lrm::with_lrm_type(Some(&l), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    let km = Lrm::with_unit_of_measure(Some(&l), Some(KILOMETRE.to_owned()), &all).unwrap().unwrap();
    assert_eq!(km.unit_of_measure(), KILOMETRE);
    assert_eq!(state(Lrm::with_unit_of_measure(Some(&l), None, &all)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(Lrm::with_unit_of_measure(Some(&l), Some("ell".to_owned()), &all)), SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED);
    assert_eq!(Lrm::with_unit_of_measure(None, Some("ell".to_owned()), &all).unwrap(), None);
    let c = Lrm::with_constraints(Some(&l), Some(vec!["a".to_owned()])).unwrap();
    assert_eq!(c.constraints(), Some(&["a".to_owned()][..]));
    assert_eq!(Lrm::with_constraints(None, None), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.8 to §15.1.10
/// Description 4] setting the offset unit sets default directions,
/// clearing it clears them, and a direction needs an offset unit.
#[test]
fn lrm_offset_mutators() {
    let all = units();
    let l = lrm(1, ABSOLUTE);
    assert_eq!(state(Lrm::with_pos_lat_offset_dir(Some(&l), Some(LEFT.to_owned()))), SqlState::SPATIAL_OFFSET_UNIT_MUST_BE_SPECIFIED);
    assert_eq!(state(Lrm::with_pos_ver_offset_dir(Some(&l), Some(DOWN.to_owned()))), SqlState::SPATIAL_OFFSET_UNIT_MUST_BE_SPECIFIED);
    assert_eq!(Lrm::with_pos_lat_offset_dir(Some(&l), None).unwrap().unwrap().pos_lat_offset_dir(), None);
    let o = Lrm::with_offset_meas_unit(Some(&l), Some(METRE.to_owned()), &all).unwrap().unwrap();
    assert_eq!((o.offset_meas_unit(), o.pos_lat_offset_dir(), o.pos_ver_offset_dir()), (Some(METRE), Some(RIGHT), Some(UP)));
    let left = Lrm::with_pos_lat_offset_dir(Some(&o), Some(LEFT.to_owned())).unwrap().unwrap();
    assert_eq!(left.pos_lat_offset_dir(), Some(LEFT));
    assert_eq!(Lrm::with_pos_lat_offset_dir(Some(&left), None).unwrap().unwrap().pos_lat_offset_dir(), Some(RIGHT));
    let down = Lrm::with_pos_ver_offset_dir(Some(&o), Some(DOWN.to_owned())).unwrap().unwrap();
    assert_eq!(down.pos_ver_offset_dir(), Some(DOWN));
    assert_eq!(Lrm::with_pos_ver_offset_dir(Some(&down), None).unwrap().unwrap().pos_ver_offset_dir(), Some(UP));
    let cleared = Lrm::with_offset_meas_unit(Some(&o), None, &all).unwrap().unwrap();
    assert_eq!((cleared.offset_meas_unit(), cleared.pos_lat_offset_dir(), cleared.pos_ver_offset_dir()), (None, None, None));
    assert_eq!(state(Lrm::with_offset_meas_unit(Some(&l), Some("rod".to_owned()), &all)), SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED);
    assert_eq!(Lrm::with_offset_meas_unit(None, None, &all).unwrap(), None);
    assert_eq!(Lrm::with_pos_ver_offset_dir(None, Some(UP.to_owned())).unwrap(), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.3.2 Description
/// 4, 6, §15.4.2 Description 4, §15.5.2 Description 4] each element
/// constructor refuses a null required argument; a feature's referents
/// default to ARRAY[]; a curve must be an ST_Curve.
#[test]
fn element_constructors() {
    let f = LrFeature::new(parts(1, 1), Some("F".to_owned())).unwrap();
    assert_eq!((f.feature_id(), f.referents()), ("F", Some(&[][..])));
    assert_eq!(LrFeature::new_with_referents(parts(1, 1), Some("F".to_owned()), None).unwrap().referents(), None);
    assert_eq!(state(LrFeature::new(parts(1, 1), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(LrFeature::new(ElementParts { id: None, ..parts(1, 1) }, Some("F".to_owned()))), SqlState::SPATIAL_NULL_ARGUMENT);
    let no_lrm = ElementParts { default_lrm: None, ..parts(1, 1) };
    assert_eq!(state(LrFeature::new_with_referents(no_lrm, Some("F".to_owned()), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    let no_measure = ElementParts { default_measure: None, ..parts(1, 1) };
    assert_eq!(state(LrCurve::new(no_measure, Some(point(0.0_f64, 0.0_f64)))), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(LrCurve::new(parts(1, 1), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(LrCurve::new(parts(1, 1), Some(point(0.0_f64, 0.0_f64)))), SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE);
    let edge = EdgeParts { topology_type: Some('E'), topology_name: Some("net".to_owned()), edge_id: Some(4_i32) };
    let e = LrDirectedEdge::new(parts(1, 1), edge.clone()).unwrap();
    assert_eq!((e.topology_type(), e.topo_or_net_name(), e.edge_or_link_id()), ('E', "net", 4_i32));
    for broken in [
        EdgeParts { topology_type: None, ..edge.clone() },
        EdgeParts { topology_name: None, ..edge.clone() },
        EdgeParts { edge_id: None, ..edge.clone() },
    ] {
        assert_eq!(state(LrDirectedEdge::new(parts(1, 1), broken)), SqlState::SPATIAL_NULL_ARGUMENT);
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.3.3, §15.3.4,
/// §15.4.3, §15.5.3 to §15.5.5 Description 4] the subtype mutators.
#[test]
fn subtype_mutators() {
    let f = LrFeature::new(parts(1, 1), Some("F".to_owned())).unwrap();
    assert_eq!(LrFeature::with_feature_id(Some(&f), Some("G".to_owned())).unwrap().unwrap().feature_id(), "G");
    assert_eq!(state(LrFeature::with_feature_id(Some(&f), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(LrFeature::with_referents(Some(&f), None).unwrap().referents(), None);
    let c = road();
    let other = line(&[0.0_f64, 0.0_f64, 0.0_f64, 5.0_f64], Dims::Xy);
    assert_eq!(LrCurve::with_curve(Some(&c), Some(other.clone())).unwrap().unwrap().curve(), &other);
    assert_eq!(state(LrCurve::with_curve(Some(&c), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(LrCurve::with_curve(None, Some(point(1.0_f64, 1.0_f64)))), SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE);
    let edge = EdgeParts { topology_type: Some('L'), topology_name: Some("n".to_owned()), edge_id: Some(1_i32) };
    let e = LrDirectedEdge::new(parts(1, 1), edge).unwrap();
    assert_eq!(LrDirectedEdge::with_topology_type(Some(&e), Some('E')).unwrap().unwrap().topology_type(), 'E');
    assert_eq!(state(LrDirectedEdge::with_topology_type(Some(&e), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(LrDirectedEdge::with_topo_or_net_name(Some(&e), Some("m".to_owned())).unwrap().unwrap().topo_or_net_name(), "m");
    assert_eq!(state(LrDirectedEdge::with_topo_or_net_name(Some(&e), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(LrDirectedEdge::with_edge_or_link_id(Some(&e), Some(8_i32)).unwrap().unwrap().edge_or_link_id(), 8_i32);
    assert_eq!(state(LrDirectedEdge::with_edge_or_link_id(None, None)), SqlState::SPATIAL_NULL_ARGUMENT);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.2.2 to §15.2.6]
/// the linear element mutators; changing the default measure raises the
/// warning 01F82.
#[test]
fn linear_element_mutators() {
    let e = LinearElement::Curve(road());
    assert_eq!(LinearElement::with_linear_element_id(Some(&e), Some(5_i32)).unwrap().unwrap().linear_element_id(), 5_i32);
    assert_eq!(state(LinearElement::with_linear_element_id(Some(&e), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(LinearElement::with_default_lrm(Some(&e), Some(2_i32)).unwrap().unwrap().default_lrm(), 2_i32);
    assert_eq!(state(LinearElement::with_default_lrm(None, None)), SqlState::SPATIAL_NULL_ARGUMENT);
    let (changed, warning) = LinearElement::with_default_measure(Some(&e), Some(m(7.0_f64))).unwrap().unwrap();
    assert_eq!((changed.default_measure(), warning), (&m(7.0_f64), SqlState::SPATIAL_CHANGING_DEFAULT_MEASURE));
    assert_eq!(state(LinearElement::with_default_measure(Some(&e), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(LinearElement::with_le_type(Some(&e), None).unwrap().le_type(), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.2.5, §15.2.6] a
/// start value defaults to 0 in the LRM's unit, and setting one for an
/// LRM replaces that LRM's.
#[test]
fn start_values() {
    let e = LinearElement::Curve(road());
    let lrms = [lrm(1, ABSOLUTE), lrm(2, RELATIVE)];
    assert_eq!(e.start_value(Some(2_i32), &lrms).unwrap(), m(0.0_f64));
    assert_eq!(state(e.start_value(None, &lrms)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(e.start_value(Some(3_i32), &lrms)), SqlState::SPATIAL_INVALID_LRM);
    let s = LinearElement::with_start_value(Some(&e), Some(2_i32), Some(m(4.0_f64))).unwrap().unwrap();
    assert_eq!(s.start_value(Some(2_i32), &lrms).unwrap(), m(4.0_f64));
    let s = LinearElement::with_start_value(Some(&s), Some(2_i32), Some(m(6.0_f64))).unwrap().unwrap();
    assert_eq!(s.base().start_values.as_ref().map(Vec::len), Some(1));
    assert_eq!(s.start_value(Some(2_i32), &lrms).unwrap(), m(6.0_f64));
    assert_eq!(state(LinearElement::with_start_value(Some(&e), None, Some(m(1.0_f64)))), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(LinearElement::with_start_value(Some(&e), Some(1_i32), None)), SqlState::SPATIAL_NULL_ARGUMENT);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.6.2 to §15.6.7]
/// the four position expression constructors and the mutators, a value
/// setting the id it carries.
#[test]
fn position_expressions() {
    let d = DistanceExp::along(m(1.0_f64));
    let e = LinearElement::Curve(road());
    let p = PositionExp::of_ids(1, 2, d.clone());
    assert_eq!((p.linear_element_id(), p.lrm_id(), p.linear_element(), p.lrm()), (1_i32, 2_i32, None, None));
    assert_eq!(PositionExp::of_id_and_lrm(1, lrm(3, ABSOLUTE), d.clone()).lrm_id(), 3_i32);
    assert_eq!(PositionExp::of_element_and_id(e.clone(), 3, d.clone()).linear_element_id(), 1_i32);
    let full = PositionExp::of_element_and_lrm(e.clone(), lrm(4, ABSOLUTE), d.clone());
    assert_eq!((full.linear_element_id(), full.lrm_id()), (1_i32, 4_i32));
    assert_eq!(PositionExp::with_linear_element_id(Some(&p), Some(8_i32)).unwrap().unwrap().linear_element_id(), 8_i32);
    assert_eq!(state(PositionExp::with_linear_element_id(Some(&p), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    let other = LinearElement::with_linear_element_id(Some(&e), Some(6_i32)).unwrap().unwrap();
    assert_eq!(PositionExp::with_linear_element(Some(&p), Some(other)).unwrap().linear_element_id(), 6_i32);
    assert_eq!(PositionExp::with_linear_element(Some(&full), None).unwrap().linear_element_id(), 1_i32);
    assert_eq!(PositionExp::with_lrm(Some(&p), Some(lrm(7, ABSOLUTE))).unwrap().lrm_id(), 7_i32);
    assert_eq!(PositionExp::with_lrm(Some(&full), None).unwrap().lrm_id(), 4_i32);
    assert_eq!(PositionExp::with_lrm_id(Some(&p), Some(5_i32)).unwrap().unwrap().lrm_id(), 5_i32);
    assert_eq!(state(PositionExp::with_lrm_id(None, None)), SqlState::SPATIAL_NULL_ARGUMENT);
    let d2 = DistanceExp::along(m(2.0_f64));
    assert_eq!(PositionExp::with_distance_exp(Some(&p), Some(d2.clone())).unwrap().unwrap().distance_exp(), &d2);
    assert_eq!(state(PositionExp::with_distance_exp(Some(&p), None)), SqlState::SPATIAL_NULL_ARGUMENT);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.7.2 to §15.7.4,
/// §15.8.2 to §15.8.4] measures and start values: a unit, when given, is
/// checked; the null unit is allowed.
#[test]
fn measures_and_start_values() {
    let all = units();
    assert_eq!(LrMeasure::new(3.0_f64).unit_of_measure(), None);
    let k = LrMeasure::new_with_units(Some(2.0_f64), Some(KILOMETRE.to_owned()), &all).unwrap();
    assert_eq!((k.measure(), k.unit_of_measure()), (2.0_f64, Some(KILOMETRE)));
    assert_eq!(state(LrMeasure::new_with_units(None, None, &all)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(
        state(LrMeasure::new_with_units(Some(1.0_f64), Some("pace".to_owned()), &all)),
        SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED
    );
    assert_eq!(LrMeasure::with_measure(Some(&k), Some(5.0_f64)).unwrap().unwrap().measure(), 5.0_f64);
    assert_eq!(state(LrMeasure::with_measure(Some(&k), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(LrMeasure::with_unit_of_measure(Some(&k), None, &all).unwrap().unwrap().unit_of_measure(), None);
    assert_eq!(
        state(LrMeasure::with_unit_of_measure(Some(&k), Some("pace".to_owned()), &all)),
        SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED
    );
    assert_eq!(LrMeasure::with_unit_of_measure(None, None, &all).unwrap(), None);
    let s = StartValue::new(1, m(2.0_f64));
    assert_eq!((s.lrm(), s.measure()), (1_i32, &m(2.0_f64)));
    assert_eq!(StartValue::with_lrm(Some(&s), Some(3_i32)).unwrap().unwrap().lrm(), 3_i32);
    assert_eq!(state(StartValue::with_lrm(Some(&s), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(StartValue::with_measure(Some(&s), Some(m(9.0_f64))).unwrap().unwrap().measure(), &m(9.0_f64));
    assert_eq!(state(StartValue::with_measure(None, None)), SqlState::SPATIAL_NULL_ARGUMENT);
}

/// The referent names "from" `from` alone.
fn from(name: &str) -> ReferentNames {
    ReferentNames { from_name: Some(name.to_owned()), ..ReferentNames::default() }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9.2 Description
/// 30, 32, §15.9.3 to §15.9.7] distance expressions: a towards referent
/// needs a from referent, in the constructor and the mutator.
#[test]
fn distance_expressions() {
    let towards = ReferentNames { towards_name: Some("b".to_owned()), ..ReferentNames::default() };
    assert_eq!(state(DistanceExp::new(Some(m(1.0_f64)), towards.clone(), None, None)), SqlState::SPATIAL_TOWARDS_REFERENT_REQUIRES_FROM);
    assert_eq!(state(DistanceExp::new(None, from("a"), None, None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(DistanceExp::new_vector(None, towards.clone(), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert!(DistanceExp::new_vector(Some(m(1.0_f64)), towards, None).is_ok());
    let d = DistanceExp::new(Some(m(1.0_f64)), from("a"), None, None).unwrap();
    assert!(d.has_referent() && !d.has_offset());
    assert_eq!((d.distance_along(), d.from_ref_name(), d.from_ref_fea_id()), (&m(1.0_f64), Some("a"), None));
    let t = DistanceExp::with_towards_ref_name(Some(&d), Some("b".to_owned())).unwrap().unwrap();
    assert_eq!(t.towards_ref_name(), Some("b"));
    let plain = DistanceExp::along(m(1.0_f64));
    assert_eq!(
        state(DistanceExp::with_towards_ref_name(Some(&plain), Some("b".to_owned()))),
        SqlState::SPATIAL_TOWARDS_REFERENT_REQUIRES_FROM
    );
    assert_eq!(state(DistanceExp::with_towards_ref_name(None, Some("b".to_owned()))), SqlState::SPATIAL_TOWARDS_REFERENT_REQUIRES_FROM);
    assert_eq!(DistanceExp::with_towards_ref_name(None, None).unwrap(), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9.3 to §15.9.6]
/// the mutators of a distance expression's distance and referents.
#[test]
fn distance_mutators() {
    let d = DistanceExp::new(Some(m(1.0_f64)), from("a"), None, None).unwrap();
    assert_eq!(DistanceExp::with_distance_along(Some(&d), Some(m(2.0_f64))).unwrap().unwrap().distance_along(), &m(2.0_f64));
    assert_eq!(state(DistanceExp::with_distance_along(Some(&d), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(DistanceExp::with_from_ref_fea_id(Some(&d), Some("F".to_owned())).unwrap().from_ref_fea_id(), Some("F"));
    assert_eq!(DistanceExp::with_from_ref_name(Some(&d), None).unwrap().from_ref_name(), None);
    assert_eq!(DistanceExp::with_towards_ref_fea_id(Some(&d), Some("G".to_owned())).unwrap().towards_ref_fea_id(), Some("G"));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9.8 to §15.9.10
/// Description 4] a vector offset excludes lateral and vertical ones.
#[test]
fn offsets_exclude_each_other() {
    let lat = LatOffsetExp::new(m(1.0_f64));
    let ver = VerOffsetExp::new(m(1.0_f64));
    let vec = VectorOffsetExp::new(vec![Vector::xy(1.0_f64, 2.0_f64)]).unwrap();
    let d = DistanceExp::along(m(1.0_f64));
    let with_lat = DistanceExp::with_lat_offset_exp(Some(&d), Some(lat.clone())).unwrap().unwrap();
    assert_eq!(with_lat.lat_offset_exp(), Some(&lat));
    assert_eq!(
        state(DistanceExp::with_vector_offset_exp(Some(&with_lat), Some(vec.clone()))),
        SqlState::SPATIAL_ILLEGAL_WITH_LATERAL_OFFSET
    );
    let with_ver = DistanceExp::with_ver_offset_exp(Some(&d), Some(ver.clone())).unwrap().unwrap();
    assert_eq!(with_ver.ver_offset_exp(), Some(&ver));
    assert_eq!(
        state(DistanceExp::with_vector_offset_exp(Some(&with_ver), Some(vec.clone()))),
        SqlState::SPATIAL_ILLEGAL_WITH_VERTICAL_OFFSET
    );
    let vectored = DistanceExp::with_vector_offset_exp(Some(&d), Some(vec.clone())).unwrap().unwrap();
    assert_eq!(vectored.vector_offset_exp(), Some(&vec));
    assert_eq!(state(DistanceExp::with_lat_offset_exp(Some(&vectored), Some(lat))), SqlState::SPATIAL_ILLEGAL_WITH_VECTOR_OFFSET);
    assert_eq!(state(DistanceExp::with_ver_offset_exp(Some(&vectored), Some(ver))), SqlState::SPATIAL_ILLEGAL_WITH_VECTOR_OFFSET);
    assert_eq!(DistanceExp::with_lat_offset_exp(None, None).unwrap(), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.11.2 to §15.11.5,
/// §15.12.2 to §15.12.5] an offset has a feature geometry or a
/// description, not both.
#[test]
fn offset_expressions() {
    let g = point(1.0_f64, 1.0_f64);
    let by_desc = LatOffsetExp::with_description(m(2.0_f64), "kerb".to_owned());
    assert_eq!((by_desc.offset_lat_dist(), by_desc.offset_ref_desc()), (Some(&m(2.0_f64)), Some("kerb")));
    assert_eq!(
        state(LatOffsetExp::with_feature_geometry(Some(&by_desc), Some(g.clone()))),
        SqlState::SPATIAL_ILLEGAL_WITH_OFFSET_REFERENT_DESCRIPTION
    );
    let by_geom = LatOffsetExp::with_geometry(m(2.0_f64), g.clone());
    assert_eq!(by_geom.feature_geometry(), Some(&g));
    assert_eq!(
        state(LatOffsetExp::with_offset_ref_desc(Some(&by_geom), Some("x".to_owned()))),
        SqlState::SPATIAL_ILLEGAL_WITH_OFFSET_REFERENT_GEOMETRY
    );
    assert_eq!(LatOffsetExp::with_offset_lat_dist(Some(&by_geom), None).unwrap().offset_lat_dist(), None);
    assert!(LatOffsetExp::with_offset_ref_desc(Some(&by_geom), None).is_ok());
    let v_desc = VerOffsetExp::with_description(m(2.0_f64), "deck".to_owned());
    assert_eq!(
        state(VerOffsetExp::with_feature_geometry(Some(&v_desc), Some(g.clone()))),
        SqlState::SPATIAL_ILLEGAL_WITH_OFFSET_REFERENT_DESCRIPTION
    );
    let v_geom = VerOffsetExp::with_geometry(m(2.0_f64), g.clone());
    assert_eq!(
        state(VerOffsetExp::with_offset_ref_desc(Some(&v_geom), Some("y".to_owned()))),
        SqlState::SPATIAL_ILLEGAL_WITH_OFFSET_REFERENT_GEOMETRY
    );
    assert_eq!(VerOffsetExp::with_offset_ver_dist(Some(&v_geom), Some(m(3.0_f64))).unwrap().offset_ver_dist(), Some(&m(3.0_f64)));
    assert_eq!(v_geom.feature_geometry(), Some(&g));
    assert_eq!(v_desc.offset_ref_desc(), Some("deck"));
    assert_eq!(VerOffsetExp::with_feature_geometry(None, Some(g)).unwrap(), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.13.1 to §15.13.3]
/// a vector offset holds at most three vectors.
#[test]
fn vector_offsets() {
    let three = vec![Vector::xy(1.0_f64, 0.0_f64); MAX_VECTORS];
    let v = VectorOffsetExp::new(three.clone()).unwrap();
    assert_eq!(v.vectors().len(), MAX_VECTORS);
    let mut four = three;
    four.push(Vector::xyz(0.0_f64, 0.0_f64, 1.0_f64));
    assert_eq!(state(VectorOffsetExp::new(four.clone())), SqlState::ARRAY_DATA_RIGHT_TRUNCATION);
    assert_eq!(state(VectorOffsetExp::with_vectors(Some(&v), Some(four))), SqlState::ARRAY_DATA_RIGHT_TRUNCATION);
    assert_eq!(state(VectorOffsetExp::with_vectors(Some(&v), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(VectorOffsetExp::with_vectors(None, Some(Vec::new())).unwrap(), None);
    assert!(Vector { x: None, y: None, z: None }.is_empty());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.10.2 to §15.10.7]
/// referents: names and types are required; the mutators of name and type
/// refuse the null value; a position alone or a location alone is kept.
#[test]
fn referents() {
    let all = units();
    let cat = Catalog::of_units(&all);
    let r = Referent::new(Some("km0".to_owned()), Some("post".to_owned()), Some(point(1.0_f64, 2.0_f64)), None, &cat).unwrap();
    assert_eq!((r.referent_name(), r.referent_type(), r.location()), ("km0", "post", None));
    assert_eq!(state(Referent::new(None, Some("t".to_owned()), None, None, &cat)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(Referent::new(Some("n".to_owned()), None, None, None, &cat)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(Referent::with_referent_name(Some(&r), Some("a".to_owned())).unwrap().unwrap().referent_name(), "a");
    assert_eq!(state(Referent::with_referent_name(Some(&r), None)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(Referent::with_referent_type(Some(&r), Some("b".to_owned())).unwrap().unwrap().referent_type(), "b");
    assert_eq!(state(Referent::with_referent_type(None, None)), SqlState::SPATIAL_NULL_ARGUMENT);
    let moved = Referent::with_position(Some(&r), Some(point(3.0_f64, 4.0_f64)), &cat).unwrap().unwrap();
    assert_eq!(moved.position(), Some(&point(3.0_f64, 4.0_f64)));
    let at = PositionExp::of_ids(1, 1, DistanceExp::along(m(1.0_f64)));
    let located = Referent::change_pos_and_loc(Some(&r), None, Some(at.clone()), &cat).unwrap().unwrap();
    assert_eq!((located.position(), located.location()), (None, Some(&at)));
    assert_eq!(Referent::with_location(None, Some(at), &cat).unwrap(), None);
}
