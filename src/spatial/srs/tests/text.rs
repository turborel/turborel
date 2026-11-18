//! Requirements of the <spatial reference system> text: reading each
//! production, writing the canonical form, and refusing malformed text
//! with F22.

use super::super::Condition;
use super::super::model::{CoordinateSystem, Direction};
use super::super::wkt;
use super::system;

const WGS84: &str = r#"GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;

fn refused(text: &str) {
    let fault = wkt::read(text).expect_err(text);
    assert_eq!(fault.condition, Condition::InvalidText, "{fault}");
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9 Description 1a, 1c] key words
/// in any case, either delimiter pair, white space between tokens, and
/// the canonical text with brackets, upper-case key words and shortest
/// numbers, which reads back to an equal model.
#[test]
fn reads_any_case_and_delimiters_and_writes_canonically() {
    let loose = r#"geogcs ( "WGS 84" , datum["WGS_1984", spheroid("WGS 84", 6378137.000, 298.257223563)], primem["Greenwich", 0.0], unit["degree", 1.74532925199433E-2] )"#;
    let read = system(loose);
    assert_eq!(wkt::write(&read), WGS84);
    assert_eq!(system(&wkt::write(&read)), read);
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9 Description 1b] the three
/// kinds of system, with the AUTHORITY, TOWGS84 and AXIS extensions and a
/// geographic system's linear unit, round trip through the canonical text.
#[test]
fn every_kind_and_extension_round_trips() {
    let texts = [
        r#"PROJCS["UTM 32N",GEOGCS["ETRS89",DATUM["ETRS_1989",SPHEROID["GRS 1980",6378137,298.257222101,AUTHORITY["EPSG","7019"]],TOWGS84[0,0,0,0,0,0,0],AUTHORITY["EPSG","6258"]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433],AXIS["Lat",NORTH],AXIS["Lon",EAST]],PROJECTION["Transverse_Mercator",AUTHORITY["EPSG","9807"]],PARAMETER["central_meridian",9],PARAMETER["scale_factor",0.9996],PARAMETER["false_easting",500000],UNIT["metre",1],AXIS["E",EAST],AXIS["N",NORTH],AUTHORITY["EPSG","25832"]]"#,
        r#"GEOGCS["3D",DATUM["D",SPHEROID["S",6378137,0],TOWGS84[1,2,3]],PRIMEM["Paris",2.33722917],UNIT["grad",0.015707963267949],UNIT["foot",0.3048],AXIS["Lon",EAST],AXIS["Lat",NORTH],AXIS["H",UP]]"#,
        r#"GEOCCS["ECEF",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["metre",1],AXIS["X",OTHER],AXIS["Y",EAST],AXIS["Z",NORTH]]"#,
    ];
    for text in texts {
        let read = system(text);
        assert_eq!(wkt::write(&read), text.replace("TOWGS84[1,2,3]", "TOWGS84[1,2,3,0,0,0,0]"));
    }
    let CoordinateSystem::Geographic(geographic) = system(texts[1]) else { panic!("not geographic") };
    assert_eq!(geographic.linear_unit.map(|unit| unit.factor), Some(0.3048_f64));
    assert_eq!(geographic.axes.last().map(|axis| axis.direction), Some(Direction::Up));
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9 Description 1a, 1k, 1l, 1m]
/// F22 for mismatched delimiters, unknown and misplaced items, missing
/// arguments, a semi-major axis or unit factor not above zero, an inverse
/// flattening in (0, 1], bad TOWGS84 and AXIS items, trailing text and
/// nesting deeper than the grammar.
#[test]
fn malformed_text_raises_invalid_text() {
    let cases = [
        "",
        "GEOGCS",
        r#"GEOGCS["x",DATUM["d",SPHEROID["s",6378137,298.257223563]),PRIMEM["G",0],UNIT["degree",0.0174532925199433]]"#,
        r#"LOCAL_CS["x"]"#,
        r#"GEOGCS["x",PRIMEM["G",0],DATUM["d",SPHEROID["s",6378137,298.257223563]],UNIT["degree",0.0174532925199433]]"#,
        r#"GEOGCS["x",DATUM["d",SPHEROID["s",0,298.257223563]],PRIMEM["G",0],UNIT["degree",0.0174532925199433]]"#,
        r#"GEOGCS["x",DATUM["d",SPHEROID["s",6378137,0.5]],PRIMEM["G",0],UNIT["degree",0.0174532925199433]]"#,
        r#"GEOGCS["x",DATUM["d",SPHEROID["s",6378137,298.257223563]],PRIMEM["G",0],UNIT["degree",0]]"#,
        r#"GEOGCS["x",DATUM["d",SPHEROID["s",6378137,298.257223563],TOWGS84[1,2]],PRIMEM["G",0],UNIT["degree",0.0174532925199433]]"#,
        r#"GEOGCS["x",DATUM["d",SPHEROID["s",6378137,298.257223563]],PRIMEM["G",0],UNIT["degree",0.0174532925199433],AXIS["a",SIDEWAYS],AXIS["b",NORTH]]"#,
        r#"GEOGCS["x",DATUM["d",SPHEROID["s",6378137,298.257223563]],PRIMEM["G",0],UNIT["degree",0.0174532925199433],AXIS["a",EAST]]"#,
        r#"GEOGCS["x",DATUM["d",SPHEROID["s",6378137,298.257223563]],PRIMEM["G",0],UNIT["degree",0.0174532925199433]] extra"#,
        r#"GEOGCS["x",DATUM["d",SPHEROID["s",6378137,298.257223563]],PRIMEM["G",0],UNIT["degree",0.0174532925199433],42]"#,
        r#"GEOGCS["x",DATUM["d",SPHEROID["s",6378137,298.257223563,AUTHORITY["E",AUTHORITY["F","1"]]]],PRIMEM["G",0],UNIT["degree",1]]"#,
        r#"GEOGCS["unterminated"#,
    ];
    for text in cases {
        refused(text);
    }
}
