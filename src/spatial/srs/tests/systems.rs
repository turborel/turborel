//! Requirements of ST_SpatialRefSys over the systems the catalog supplies.

use super::super::Condition;
use super::super::system::{SpatialRefSys, UNSPECIFIED};
use super::super::wkt;
use super::{near, seed};

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.2 Description 5, §14.1.3
/// Description 2] every system the catalog supplies reads, its canonical
/// text reads back to the same model, its SRID is its identifier, and it
/// transforms a point of its area from WGS 84 and back.
#[test]
fn every_seed_reads_and_round_trips() {
    for row in crate::system::SRS_SEED.iter() {
        let srid = row.id;
        let system = SpatialRefSys::from_srid(srid).unwrap_or_else(|fault| panic!("{srid}: {fault}"));
        let canonical = system.as_wkt().unwrap_or_default();
        assert_eq!(wkt::read(&canonical).as_ref(), Ok(system.definition().unwrap()), "{srid}");
        assert_eq!(wkt::read(&row.definition).as_ref(), Ok(system.definition().unwrap()), "{srid}");
        assert_eq!(system.srid(), Some(srid));
        let wgs84 = SpatialRefSys::from_srid(4326).unwrap();
        let there = wgs84.transform_to(&system).unwrap_or_else(|fault| panic!("{srid}: {fault}"));
        let back = system.transform_to(&wgs84).unwrap();
        let point = sample(srid);
        let mut coordinates = [point[0], point[1], 10.0_f64];
        there.apply_all(&mut coordinates, 3, true).unwrap_or_else(|fault| panic!("{srid}: {fault}"));
        back.apply_all(&mut coordinates, 3, true).unwrap();
        near(coordinates, [point[0], point[1], 10.0_f64], 1e-6_f64);
    }
}

/// A longitude and latitude inside the area of seed `srid`.
fn sample(srid: i32) -> [f64; 2] {
    let zone = |base: i32| f64::from(srid.saturating_sub(base)) * 6.0_f64 - 183.0_f64 + 1.0_f64;
    match srid {
        32601..=32660 => [zone(32600), 40.0_f64],
        32701..=32760 => [zone(32700), -40.0_f64],
        _ => [10.0_f64, 40.0_f64],
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.5 Description 2, §14.1.2
/// Description 5] SRID 0 is the unspecified plane with no text and no
/// transform; an unknown SRID raises F59; text without an EPSG AUTHORITY
/// has no SRID until one is given.
#[test]
fn identifiers_and_the_unspecified_plane() {
    let plane = SpatialRefSys::from_srid(UNSPECIFIED).unwrap();
    assert_eq!(plane.srid(), Some(0_i32));
    assert!(plane.as_wkt().is_none() && plane.definition().is_none());
    let wgs84 = SpatialRefSys::from_srid(4326).unwrap();
    assert_eq!(plane.transform_to(&wgs84).map(|_| ()).map_err(|fault| fault.condition), Err(Condition::FailedToTransform));
    assert_eq!(wgs84.transform_to(&plane).map(|_| ()).map_err(|fault| fault.condition), Err(Condition::FailedToTransform));
    assert_eq!(SpatialRefSys::from_srid(999_999).map_err(|fault| fault.condition), Err(Condition::UnknownSystem));
    let anonymous = SpatialRefSys::from_text(
        r#"GEOGCS["x",DATUM["WGS_1984",SPHEROID["s",6378137,298.257223563]],PRIMEM["G",0],UNIT["d",0.0174532925199433]]"#,
    )
    .unwrap();
    assert_eq!(anonymous.srid(), None);
    assert_eq!(anonymous.clone().with_srid(100_000_i32).srid(), Some(100_000_i32));
    assert_eq!(SpatialRefSys::from_text("nonsense").map_err(|fault| fault.condition), Err(Condition::InvalidText));
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.6 Description 3, §14.1.7
/// Description 2] equality ignores names and authorities but not the
/// datum, ellipsoid, units, axes or projection; projection names and
/// parameter names that mean the same compare equal.
#[test]
fn equality_compares_definitions() {
    let wgs84 = SpatialRefSys::from_srid(4326).unwrap();
    let renamed = SpatialRefSys::from_text(
        r#"GEOGCS["x",DATUM["WGS 1984",SPHEROID["s",6378137,298.257223563]],PRIMEM["G",0],UNIT["d",0.0174532925199433]]"#,
    )
    .unwrap();
    assert!(wgs84.equals(&renamed) && renamed.equals(&wgs84));
    assert_eq!(wgs84.ordering_equals(&renamed), 0_i32);
    let swapped = SpatialRefSys::from_text(r#"GEOGCS["x",DATUM["WGS_1984",SPHEROID["s",6378137,298.257223563]],PRIMEM["G",0],UNIT["d",0.0174532925199433],AXIS["a",NORTH],AXIS["b",EAST]]"#).unwrap();
    let defaulted = SpatialRefSys::from_text(r#"GEOGCS["x",DATUM["WGS_1984",SPHEROID["s",6378137,298.257223563]],PRIMEM["G",0],UNIT["d",0.0174532925199433],AXIS["a",EAST],AXIS["b",NORTH],AXIS["h",UP]]"#).unwrap();
    assert!(!wgs84.equals(&swapped) && wgs84.equals(&defaulted));
    assert_eq!(wgs84.ordering_equals(&swapped), 1_i32);
    let etrs89 = SpatialRefSys::from_srid(4258).unwrap();
    let ecef = SpatialRefSys::from_srid(4978).unwrap();
    let plane = SpatialRefSys::unspecified();
    assert!(!wgs84.equals(&etrs89) && !wgs84.equals(&ecef) && !wgs84.equals(&plane) && plane.equals(&SpatialRefSys::unspecified()));
    assert!(ecef.equals(&SpatialRefSys::from_text(&seed(4978).replace("WGS 84 (geocentric)", "X")).unwrap()));
    let utm = SpatialRefSys::from_srid(32632).unwrap();
    let respelled =
        seed(32632).replace("Transverse_Mercator", "Transverse Mercator").replace("central_meridian", "longitude_of_natural_origin");
    assert!(utm.equals(&SpatialRefSys::from_text(&respelled).unwrap()));
    assert!(!utm.equals(&SpatialRefSys::from_srid(32633).unwrap()) && !utm.equals(&wgs84));
    let feet = seed(32632).replace(r#"UNIT["metre",1]"#, r#"UNIT["foot",0.3048]"#);
    assert!(!utm.equals(&SpatialRefSys::from_text(&feet).unwrap()));
    let unsupported = seed(32632).replace("Transverse_Mercator", "Bonne");
    let other = SpatialRefSys::from_text(&unsupported).unwrap();
    assert!(other.equals(&other.clone()) && !utm.equals(&other));
    assert!(wkt::read(&unsupported).is_ok());
}
