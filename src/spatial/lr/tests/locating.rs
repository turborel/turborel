//! Tests of locating positions along a curve [ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §15.4.4, §15.4.5, §15.6.8, §15.9.1, §15.10.2] and of
//! translating them [§15.2.7, §15.2.8], each against a place worked by
//! hand on the road from (0 0) to (100 0).

use super::*;

/// The LRMs of the samples: 1 absolute, 2 interpolative, 3 relative, 4
/// local interpolative, 5 absolute allowing offsets to the right, 6 the
/// same to the left.
fn lrms() -> Vec<Lrm> {
    vec![
        lrm(1, ABSOLUTE),
        lrm(2, INTERPOLATIVE),
        lrm(3, RELATIVE),
        lrm(4, LOCAL_INTERPOLATIVE),
        lrm_offsets(5, ABSOLUTE, RIGHT),
        lrm_offsets(6, ABSOLUTE, LEFT),
    ]
}

/// The point ST_Point gives of `d` along the road in LRM `lrm_id`, with
/// `elements` in the catalog.
fn at(lrm_id: i32, d: DistanceExp, elements: &[LinearElement]) -> Result<Vec<f64>> {
    let all = units();
    let lrms = lrms();
    let cat = Catalog { units: &all, lrms: &lrms, elements, metres_per_unit: 1.0_f64 };
    let p = PositionExp::of_ids(1, lrm_id, d);
    road().point(Some(&p), &cat).map(|g| ordinates(&g))
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9.1 Description
/// 15, 16] an absolute distance is measured from the start, in its unit,
/// less the start value: 25 m is (25 0), 0.05 km is (50 0), 25 m from a
/// start value of 10 m is (15 0).
#[test]
fn absolute_measures() {
    assert!(near(&at(1, DistanceExp::along(m(25.0_f64)), &[]).unwrap(), &[25.0_f64, 0.0_f64]));
    let km = LrMeasure { measure: 0.05_f64, units: Some(KILOMETRE.to_owned()) };
    assert!(near(&at(1, DistanceExp::along(km), &[]).unwrap(), &[50.0_f64, 0.0_f64]));
    let unitless = LrMeasure::new(40.0_f64);
    assert!(near(&at(1, DistanceExp::along(unitless), &[]).unwrap(), &[40.0_f64, 0.0_f64]));
    let all = units();
    let lrms = lrms();
    let cat = Catalog { units: &all, lrms: &lrms, elements: &[], metres_per_unit: 1.0_f64 };
    let started = ElementParts { start_values: Some(vec![StartValue::new(1, m(10.0_f64))]), ..parts(1, 1) };
    let c = LrCurve::new(started, Some(road().curve)).unwrap();
    let p = PositionExp::of_ids(1, 1, DistanceExp::along(m(25.0_f64)));
    assert!(near(&ordinates(&c.point(Some(&p), &cat).unwrap()), &[15.0_f64, 0.0_f64]));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9.1 Description
/// 17] an interpolative distance is pro-rated by the default measure: 25
/// of 100 along a curve of 200 is 50 along it.
#[test]
fn interpolative_measures() {
    let all = units();
    let lrms = lrms();
    let cat = Catalog { units: &all, lrms: &lrms, elements: &[], metres_per_unit: 1.0_f64 };
    let long = curve(2, 2, line(&[0.0_f64, 0.0_f64, 200.0_f64, 0.0_f64], Dims::Xy));
    let p = PositionExp::of_ids(2, 2, DistanceExp::along(m(25.0_f64)));
    assert!(near(&ordinates(&long.point(Some(&p), &cat).unwrap()), &[50.0_f64, 0.0_f64]));
    let zero = ElementParts { default_measure: Some(m(0.0_f64)), ..parts(2, 2) };
    let flat = LrCurve::new(zero, Some(long.curve)).unwrap();
    assert_eq!(state(flat.point(Some(&p), &cat)), SqlState::SPATIAL_ATTEMPTED_DIVISION_BY_ZERO);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9.1 Description
/// 18] a local interpolative distance is an m value: 15 between m 10 and
/// m 20 is half way, and 30 between m 20 and m 40 is half way along the
/// second segment; a curve without m raises 2FF84.
#[test]
fn local_interpolative_measures() {
    let all = units();
    let lrms = lrms();
    let cat = Catalog { units: &all, lrms: &lrms, elements: &[], metres_per_unit: 1.0_f64 };
    let measured = curve(3, 4, line(&[0.0_f64, 0.0_f64, 10.0_f64, 100.0_f64, 0.0_f64, 20.0_f64], Dims::Xym));
    let p = PositionExp::of_ids(3, 4, DistanceExp::along(LrMeasure::new(15.0_f64)));
    assert!(near(&ordinates(&measured.point(Some(&p), &cat).unwrap()), &[50.0_f64, 0.0_f64, 15.0_f64]));
    let outside = PositionExp::of_ids(3, 4, DistanceExp::along(LrMeasure::new(25.0_f64)));
    assert_eq!(state(measured.point(Some(&outside), &cat)), SqlState::SPATIAL_DISTANCE_LONGER_THAN_CURVE);
    assert_eq!(state(at(4, DistanceExp::along(LrMeasure::new(15.0_f64)), &[])), SqlState::SPATIAL_MISSING_MEASURE_VALUES);
    let coords = [0.0_f64, 0.0_f64, 10.0_f64, 100.0_f64, 0.0_f64, 20.0_f64, 200.0_f64, 0.0_f64, 40.0_f64];
    let bent = curve(3, 4, line(&coords, Dims::Xym));
    let later = PositionExp::of_ids(3, 4, DistanceExp::along(LrMeasure::new(30.0_f64)));
    assert!(near(&ordinates(&bent.point(Some(&later), &cat).unwrap()), &[150.0_f64, 0.0_f64, 30.0_f64]));
}

/// The feature F1 of referents km0 at (20 0) and w at (5 0).
fn feature() -> LinearElement {
    let all = units();
    let cat = Catalog::of_units(&all);
    let km0 = Referent::new(Some("km0".to_owned()), Some("post".to_owned()), Some(point(20.0_f64, 0.0_f64)), None, &cat).unwrap();
    let w = Referent::new(Some("w".to_owned()), Some("post".to_owned()), Some(point(5.0_f64, 0.0_f64)), None, &cat).unwrap();
    LinearElement::Feature(LrFeature::new_with_referents(parts(9, 3), Some("F1".to_owned()), Some(vec![km0, w])).unwrap())
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9.1 Description
/// 19 to 21, 23] a relative distance is measured from its "from"
/// referent, towards its "towards" referent: 10 from km0 is (30 0), 10
/// from km0 towards w is (10 0); a referent under another LRM type, or
/// unknown, is refused.
#[test]
fn relative_measures() {
    let elements = [feature()];
    let names = ReferentNames { from_name: Some("km0".to_owned()), ..ReferentNames::default() };
    let d = DistanceExp::new(Some(m(10.0_f64)), names.clone(), None, None).unwrap();
    assert!(near(&at(3, d.clone(), &elements).unwrap(), &[30.0_f64, 0.0_f64]));
    let towards = ReferentNames { towards_name: Some("w".to_owned()), from_feature: Some("F1".to_owned()), ..names };
    let back = DistanceExp::new(Some(m(10.0_f64)), towards, None, None).unwrap();
    assert!(near(&at(3, back, &elements).unwrap(), &[10.0_f64, 0.0_f64]));
    assert!(near(&at(3, DistanceExp::along(m(10.0_f64)), &elements).unwrap(), &[10.0_f64, 0.0_f64]));
    assert_eq!(state(at(1, d.clone(), &elements)), SqlState::SPATIAL_INVALID_ARGUMENT);
    assert_eq!(state(at(3, d, &[])), SqlState::SPATIAL_INVALID_ARGUMENT);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.4.4 Description
/// 2, §15.9.1 Description 31] ST_Point's exceptions: null argument,
/// another element, an unknown LRM, a distance past the curve.
#[test]
fn point_exceptions() {
    let all = units();
    let lrms = lrms();
    let cat = Catalog { units: &all, lrms: &lrms, elements: &[], metres_per_unit: 1.0_f64 };
    assert_eq!(state(road().point(None, &cat)), SqlState::SPATIAL_NULL_ARGUMENT);
    let other = PositionExp::of_ids(2, 1, DistanceExp::along(m(1.0_f64)));
    assert_eq!(state(road().point(Some(&other), &cat)), SqlState::SPATIAL_INVALID_ARGUMENT);
    assert_eq!(state(at(99, DistanceExp::along(m(1.0_f64)), &[])), SqlState::SPATIAL_INVALID_LRM);
    assert_eq!(state(at(1, DistanceExp::along(m(150.0_f64)), &[])), SqlState::SPATIAL_DISTANCE_LONGER_THAN_CURVE);
    assert_eq!(state(at(1, DistanceExp::along(m(-1.0_f64)), &[])), SqlState::SPATIAL_DISTANCE_LONGER_THAN_CURVE);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9.1 Description
/// 25 to 30] offsets: 3 m lateral at 50 m is (50 -3) to the right, (50 3)
/// to the left; 2 m vertical gives z 2; the vector (1 2) gives (51 2); an
/// LRM of no offsets refuses them.
#[test]
fn offsets_move_the_place() {
    let lateral = DistanceExp::new(Some(m(50.0_f64)), ReferentNames::default(), Some(LatOffsetExp::new(m(3.0_f64))), None).unwrap();
    assert!(near(&at(5, lateral.clone(), &[]).unwrap(), &[50.0_f64, -3.0_f64]));
    assert!(near(&at(6, lateral.clone(), &[]).unwrap(), &[50.0_f64, 3.0_f64]));
    assert_eq!(state(at(1, lateral, &[])), SqlState::SPATIAL_INVALID_ARGUMENT);
    let vertical = DistanceExp::new(Some(m(50.0_f64)), ReferentNames::default(), None, Some(VerOffsetExp::new(m(2.0_f64)))).unwrap();
    assert!(near(&at(5, vertical, &[]).unwrap(), &[50.0_f64, 0.0_f64, 2.0_f64]));
    let vectors = VectorOffsetExp::new(vec![Vector::xy(1.0_f64, 2.0_f64)]).unwrap();
    let vector = DistanceExp::new_vector(Some(m(50.0_f64)), ReferentNames::default(), Some(vectors)).unwrap();
    assert!(near(&at(5, vector, &[]).unwrap(), &[51.0_f64, 2.0_f64]));
    let kerb = LatOffsetExp::with_geometry(m(1.0_f64), line(&[0.0_f64, 5.0_f64, 100.0_f64, 5.0_f64], Dims::Xy));
    let from_kerb = DistanceExp::new(Some(m(50.0_f64)), ReferentNames::default(), Some(kerb), None).unwrap();
    assert!(near(&at(6, from_kerb, &[]).unwrap(), &[50.0_f64, 6.0_f64]));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.4.5 Description
/// 2 to 4] ST_LRPosition: (30 4) is 30 m along, and 4 m left, a lateral
/// offset of -4 under an LRM positive to the right; under an LRM of no
/// offsets it has none. Of a curve turning a corner, the nearest segment
/// gives the distance: (100 40) is 140 along.
#[test]
fn lr_position_inverts_point() {
    let all = units();
    let lrms = lrms();
    let cat = Catalog { units: &all, lrms: &lrms, elements: &[], metres_per_unit: 1.0_f64 };
    let offsets = curve(1, 5, road().curve);
    let p = offsets.lr_position(Some(&point(30.0_f64, 4.0_f64)), &cat).unwrap();
    assert_eq!((p.linear_element_id(), p.lrm_id()), (1_i32, 5_i32));
    assert!((p.distance_exp().distance_along().measure() - 30.0_f64).abs() < 1e-9_f64);
    let lateral = p.distance_exp().lat_offset_exp().and_then(LatOffsetExp::offset_lat_dist).map(LrMeasure::measure);
    assert!((lateral.unwrap() + 4.0_f64).abs() < 1e-9_f64);
    assert!(near(&ordinates(&offsets.point(Some(&p), &cat).unwrap()), &[30.0_f64, 4.0_f64]));
    let plain = road().lr_position(Some(&point(30.0_f64, 4.0_f64)), &cat).unwrap();
    assert!(plain.distance_exp().lat_offset_exp().is_none());
    let corner = curve(1, 1, line(&[0.0_f64, 0.0_f64, 100.0_f64, 0.0_f64, 100.0_f64, 100.0_f64], Dims::Xy));
    let up = corner.lr_position(Some(&point(100.0_f64, 40.0_f64)), &cat).unwrap();
    assert!((up.distance_exp().distance_along().measure() - 140.0_f64).abs() < 1e-9_f64);
    assert_eq!(state(road().lr_position(None, &cat)), SqlState::SPATIAL_NULL_ARGUMENT);
    let elsewhere = point(1.0_f64, 1.0_f64).with_srid(4326);
    assert_eq!(state(road().lr_position(Some(&elsewhere), &cat)), SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS);
    let no_lrm = curve(1, 99, road().curve);
    assert_eq!(state(no_lrm.lr_position(Some(&point(1.0_f64, 0.0_f64)), &cat)), SqlState::SPATIAL_INVALID_LRM);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.6.8 Description
/// 2] ST_Equals: 25 m and 0.025 km along the road are the same place; 30 m
/// is not; positions along an element not in the catalog compare by
/// distance if they are plain, else are indeterminate.
#[test]
fn equality() {
    let all = units();
    let lrms = lrms();
    let elements = [LinearElement::Curve(road())];
    let cat = Catalog { units: &all, lrms: &lrms, elements: &elements, metres_per_unit: 1.0_f64 };
    let km = LrMeasure { measure: 0.025_f64, units: Some(KILOMETRE.to_owned()) };
    let a = PositionExp::of_ids(1, 1, DistanceExp::along(m(25.0_f64)));
    let b = PositionExp::of_ids(1, 1, DistanceExp::along(km.clone()));
    let c = PositionExp::of_ids(1, 1, DistanceExp::along(m(30.0_f64)));
    assert_eq!(PositionExp::equals(Some(&a), Some(&b), &cat).unwrap(), Some(1_i32));
    assert_eq!(PositionExp::equals(Some(&a), Some(&c), &cat).unwrap(), Some(0_i32));
    assert_eq!(PositionExp::equals(None, Some(&c), &cat).unwrap(), None);
    assert_eq!(state(PositionExp::equals(Some(&a), None, &cat)), SqlState::SPATIAL_NULL_ARGUMENT);
    let far_a = PositionExp::of_ids(7, 1, DistanceExp::along(m(25.0_f64)));
    let far_b = PositionExp::of_ids(7, 1, DistanceExp::along(km));
    assert_eq!(PositionExp::equals(Some(&far_a), Some(&far_b), &cat).unwrap(), Some(1_i32));
    let other = PositionExp::of_ids(8, 1, DistanceExp::along(m(25.0_f64)));
    assert_eq!(state(PositionExp::equals(Some(&far_a), Some(&other), &cat)), SqlState::SPATIAL_INDETERMINATE_EQUALITY);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.10.2 Description
/// 8b] a referent's position and location must be known to agree.
#[test]
fn referent_position_and_location() {
    let all = units();
    let lrms = lrms();
    let elements = [LinearElement::Curve(road())];
    let cat = Catalog { units: &all, lrms: &lrms, elements: &elements, metres_per_unit: 1.0_f64 };
    let at25 = PositionExp::of_ids(1, 1, DistanceExp::along(m(25.0_f64)));
    let name = || Some("r".to_owned());
    assert!(Referent::new(name(), name(), Some(point(25.0_f64, 0.0_f64)), Some(at25.clone()), &cat).is_ok());
    let wrong = Referent::new(name(), name(), Some(point(26.0_f64, 0.0_f64)), Some(at25.clone()), &cat);
    assert_eq!(state(wrong), SqlState::SPATIAL_INCOMPATIBLE_REFERENT_POSITION);
    let unknown = Referent::new(name(), name(), Some(point(25.0_f64, 0.0_f64)), Some(at25.clone()), &Catalog::of_units(&all));
    assert_eq!(state(unknown), SqlState::SPATIAL_INCOMPATIBLE_REFERENT_POSITION);
    let r = Referent::new(name(), name(), None, Some(at25), &cat).unwrap();
    assert_eq!(
        state(Referent::with_position(Some(&r), Some(point(0.0_f64, 9.0_f64)), &cat)),
        SqlState::SPATIAL_INCOMPATIBLE_REFERENT_POSITION
    );
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.2.7 Description
/// 2] 25 m along the road is 5 m along a curve from (20 0); a curve 10 m
/// off it cannot hold it without offsets.
#[test]
fn translate_to_instance() {
    let all = units();
    let lrms = lrms();
    let rail = LinearElement::Curve(curve(3, 1, line(&[20.0_f64, 0.0_f64, 120.0_f64, 0.0_f64], Dims::Xy)));
    let off = LinearElement::Curve(curve(4, 1, line(&[0.0_f64, 10.0_f64, 100.0_f64, 10.0_f64], Dims::Xy)));
    let elements = [LinearElement::Curve(road())];
    let cat = Catalog { units: &all, lrms: &lrms, elements: &elements, metres_per_unit: 1.0_f64 };
    let source = PositionExp::of_ids(1, 1, DistanceExp::along(m(25.0_f64)));
    let me = LinearElement::Curve(road());
    let d = me.translate_to_inst(Some(&source), Some(&rail), None, &cat).unwrap();
    assert!((d.distance_along().measure() - 5.0_f64).abs() < 1e-9_f64);
    assert_eq!(state(me.translate_to_inst(Some(&source), Some(&off), None, &cat)), SqlState::SPATIAL_CANNOT_TRANSLATE);
    let with_offsets = me.translate_to_inst(Some(&source), Some(&off), Some(5_i32), &cat).unwrap();
    assert!(with_offsets.lat_offset_exp().is_some());
    assert_eq!(state(me.translate_to_inst(None, Some(&rail), None, &cat)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(me.translate_to_inst(Some(&source), None, None, &cat)), SqlState::SPATIAL_NULL_ARGUMENT);
    assert_eq!(state(me.translate_to_inst(Some(&source), Some(&rail), Some(99_i32), &cat)), SqlState::SPATIAL_CANNOT_TRANSLATE);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.2.8 Description
/// 2] the elements of a type that hold the place, each a position.
#[test]
fn translate_to_type() {
    let all = units();
    let lrms = lrms();
    let typed = |id: i32, coords: &[f64]| {
        let p = ElementParts { element_type: Some("rail".to_owned()), ..parts(id, 1) };
        LinearElement::Curve(LrCurve::new(p, Some(line(coords, Dims::Xy))).unwrap())
    };
    let elements = [
        LinearElement::Curve(road()),
        typed(3_i32, &[20.0_f64, 0.0_f64, 120.0_f64, 0.0_f64]),
        typed(4_i32, &[0.0_f64, 10.0_f64, 100.0_f64, 10.0_f64]),
    ];
    let cat = Catalog { units: &all, lrms: &lrms, elements: &elements, metres_per_unit: 1.0_f64 };
    let source = PositionExp::of_ids(1, 1, DistanceExp::along(m(25.0_f64)));
    let me = LinearElement::Curve(road());
    let found = me.translate_to_type(Some(&source), Some("rail"), None, &cat).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].linear_element_id(), found[0].lrm_id()), (3_i32, 1_i32));
    assert!((found[0].distance_exp().distance_along().measure() - 5.0_f64).abs() < 1e-9_f64);
    assert_eq!(state(me.translate_to_type(Some(&source), Some("canal"), None, &cat)), SqlState::SPATIAL_CANNOT_TRANSLATE);
    assert_eq!(state(me.translate_to_type(Some(&source), None, None, &cat)), SqlState::SPATIAL_NULL_ARGUMENT);
}
