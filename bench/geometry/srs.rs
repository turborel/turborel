//! The reference system engine: projecting and unprojecting points,
//! datum shifts, and geodesic distances and areas, per point.

use std::hint::black_box;

use super::harness::{Bench, Case};
use turborel::spatial::srs::geodesic::Geodesic;
use turborel::spatial::srs::system::SpatialRefSys;

/// Points per measured batch.
const POINTS: u32 = 10_000;

/// A spread of longitudes and latitudes over UTM zone 32 and France.
fn points() -> Vec<[f64; 3]> {
    (0..POINTS).map(|n| [6.0_f64 + f64::from(n % 100) * 0.06_f64, 45.0_f64 + f64::from(n / 100) * 0.1_f64, 0.0_f64]).collect()
}

/// RGF93 / Lambert-93, a user-defined system with a zero shift.
const LAMBERT_93: &str = r#"PROJCS["RGF93 / Lambert-93",GEOGCS["RGF93",DATUM["Reseau_Geodesique_Francais_1993",SPHEROID["GRS 1980",6378137,298.257222101],TOWGS84[0,0,0,0,0,0,0]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],PROJECTION["Lambert_Conformal_Conic_2SP"],PARAMETER["standard_parallel_1",49],PARAMETER["standard_parallel_2",44],PARAMETER["latitude_of_origin",46.5],PARAMETER["central_meridian",3],PARAMETER["false_easting",700000],PARAMETER["false_northing",6600000],UNIT["metre",1]]"#;

/// OSGB 1936 / British National Grid, a user-defined system with a
/// Helmert shift.
const BNG: &str = r#"PROJCS["OSGB 1936 / British National Grid",GEOGCS["OSGB 1936",DATUM["OSGB_1936",SPHEROID["Airy 1830",6377563.396,299.3249646],TOWGS84[446.448,-125.157,542.06,0.15,0.247,0.842,-20.489]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",49],PARAMETER["central_meridian",-2],PARAMETER["scale_factor",0.9996012717],PARAMETER["false_easting",400000],PARAMETER["false_northing",-100000],UNIT["metre",1]]"#;

/// A target system: a catalog SRID or a definition's text.
#[derive(Clone, Copy)]
enum Target {
    Srid(i32),
    Text(&'static str),
}

/// The transform cases from a catalog system to a target.
fn transforms(bench: &Bench, name: &str, from: i32, to: Target) {
    let from = SpatialRefSys::from_srid(from).unwrap();
    let to = match to {
        Target::Srid(srid) => SpatialRefSys::from_srid(srid).unwrap(),
        Target::Text(text) => SpatialRefSys::from_text(text).unwrap(),
    };
    let transform = from.transform_to(&to).unwrap();
    let points = points();
    bench.measure(Case::new(format!("srs {name}"), "points"), || {
        for point in &points {
            black_box(transform.apply(black_box(*point)).unwrap());
        }
        u64::from(POINTS)
    });
}

/// Every reference system case.
pub fn run(bench: &Bench) {
    transforms(bench, "4326 to UTM 32N", 4326, Target::Srid(32632));
    transforms(bench, "4326 to Pseudo-Mercator", 4326, Target::Srid(3857));
    transforms(bench, "4326 to Lambert-93, zero shift", 4326, Target::Text(LAMBERT_93));
    transforms(bench, "4326 to British National Grid, Helmert", 4326, Target::Text(BNG));
    let geodesic = Geodesic::wgs84();
    let points = points();
    bench.measure(Case::new("srs geodesic inverse", "pairs"), || {
        for pair in points.windows(2) {
            black_box(geodesic.inverse(pair[0][0], pair[0][1], pair[1][0] + 90.0_f64, -pair[1][1]));
        }
        u64::from(POINTS - 1)
    });
    let ring: Vec<[f64; 2]> = (0_i32..64_i32)
        .map(|n| f64::from(n) * std::f64::consts::TAU / 64.0_f64)
        .map(|t| [10.0_f64 + t.cos(), 50.0_f64 + t.sin()])
        .collect();
    bench.measure(Case::new("srs geodesic area, 64 vertices", "polygons"), || {
        for _ in 0_i32..100_i32 {
            black_box(geodesic.polygon(black_box(&ring)));
        }
        100
    });
}
