//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.3] The spatial reference
//! systems this implementation supplies, each with its well-known text
//! representation [§14.1.9 `<spatial reference system>`], by the
//! identifiers the EPSG registry gives them: WGS 84 geographic and
//! geocentric, NAD83, ETRS89, WGS 84 / Pseudo-Mercator, and the 120 WGS 84
//! UTM zones (RULES.md D-SPATIAL-CATALOG).

use std::sync::LazyLock;

/// The organization that defined each system supplied.
const EPSG: &str = "EPSG";

/// A spatial reference system supplied: a row of
/// ST_SPATIAL_REFERENCE_SYSTEMS.
#[derive(Debug, Clone)]
pub struct SpatialRefSys {
    /// SRS_NAME, unique.
    pub name: String,
    /// SRS_ID, unique, and ORGANIZATION_COORDSYS_ID.
    pub id: i32,
    /// ORGANIZATION.
    pub organization: &'static str,
    /// DEFINITION, its well-known text representation.
    pub definition: String,
    /// DESCRIPTION.
    pub description: String,
}

impl SpatialRefSys {
    /// The system supplied of identifier `id`, if there is one.
    #[must_use]
    pub fn by_id(id: i32) -> Option<&'static SpatialRefSys> {
        SRS_SEED.binary_search_by_key(&id, |s| s.id).ok().and_then(|at| SRS_SEED.get(at))
    }
}

/// The geographic system of WGS 84.
const WGS84: &str = r#"GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;

/// The fixed systems supplied, by identifier: name, definition and
/// description.
fn fixed() -> [(i32, String, String, &'static str); 5] {
    let grs80 = |name: &str, datum: &str| {
        format!(
            r#"GEOGCS["{name}",DATUM["{datum}",SPHEROID["GRS 1980",6378137,298.257222101]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#
        )
    };
    let mercator = format!(
        r#"PROJCS["WGS 84 / Pseudo-Mercator",{WGS84},PROJECTION["Popular_Visualisation_Pseudo_Mercator"],PARAMETER["central_meridian",0],PARAMETER["scale_factor",1],PARAMETER["false_easting",0],PARAMETER["false_northing",0],UNIT["metre",1]]"#
    );
    let geocentric =
        r#"GEOCCS["WGS 84 (geocentric)",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["metre",1]]"#;
    [
        (3857, "WGS 84 / Pseudo-Mercator".to_owned(), mercator, "web mapping's spherical Mercator projection of WGS 84"),
        (
            4258,
            "ETRS89".to_owned(),
            grs80("ETRS89", "European_Terrestrial_Reference_System_1989"),
            "European Terrestrial Reference System 1989",
        ),
        (4269, "NAD83".to_owned(), grs80("NAD83", "North_American_Datum_1983"), "North American Datum 1983"),
        (4326, "WGS 84".to_owned(), WGS84.to_owned(), "World Geodetic System 1984, longitude and latitude in degrees"),
        (4978, "WGS 84 (geocentric)".to_owned(), geocentric.to_owned(), "World Geodetic System 1984, earth-centred x, y and z in metres"),
    ]
}

/// The WGS 84 UTM zone `zone`, 1 to 60, of the northern hemisphere if
/// `north`: its identifier, name and definition.
fn utm(zone: i32, north: bool) -> SpatialRefSys {
    let (hemisphere, base, false_northing) = if north { ('N', 32_600_i32, 0_i32) } else { ('S', 32_700_i32, 10_000_000_i32) };
    let name = format!("WGS 84 / UTM zone {zone}{hemisphere}");
    let meridian = zone.saturating_mul(6).saturating_sub(183);
    let definition = format!(
        r#"PROJCS["{name}",{WGS84},PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],PARAMETER["central_meridian",{meridian}],PARAMETER["scale_factor",0.9996],PARAMETER["false_easting",500000],PARAMETER["false_northing",{false_northing}],UNIT["metre",1]]"#
    );
    let description =
        format!("Universal Transverse Mercator zone {zone}, {} hemisphere, of WGS 84", if north { "northern" } else { "southern" });
    SpatialRefSys { name, id: base.saturating_add(zone), organization: EPSG, definition, description }
}

/// Every system supplied, in order of identifier.
pub static SRS_SEED: LazyLock<Vec<SpatialRefSys>> = LazyLock::new(|| {
    let mut all: Vec<SpatialRefSys> = fixed()
        .into_iter()
        .map(|(id, name, definition, description)| SpatialRefSys {
            name,
            id,
            organization: EPSG,
            definition,
            description: description.to_owned(),
        })
        .collect();
    all.extend((1_i32..=60_i32).map(|zone| utm(zone, true)));
    all.extend((1_i32..=60_i32).map(|zone| utm(zone, false)));
    all.sort_unstable_by_key(|s| s.id);
    all
});

#[cfg(test)]
mod tests {
    use super::{SRS_SEED, SpatialRefSys};
    use crate::system::spatial::{DEFINITION_LENGTH, DESCRIPTION_LENGTH, SRS_NAME_LENGTH};

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.3] the systems
    /// supplied keep the base table's constraints: names and identifiers
    /// unique, the organization's pair given, the text no longer than its
    /// columns, each found by its identifier.
    #[test]
    fn seed_systems_keep_the_table_constraints() {
        let mut names: Vec<&str> = SRS_SEED.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SRS_SEED.len());
        assert_eq!(SRS_SEED.len(), 125);
        for s in SRS_SEED.iter() {
            assert_eq!(SpatialRefSys::by_id(s.id).map(|f| &f.name), Some(&s.name));
            assert!(s.name.chars().count() <= usize::try_from(SRS_NAME_LENGTH).unwrap());
            assert!(s.definition.chars().count() <= usize::try_from(DEFINITION_LENGTH).unwrap());
            assert!(s.description.chars().count() <= usize::try_from(DESCRIPTION_LENGTH).unwrap());
        }
        assert!(SpatialRefSys::by_id(32_633).unwrap().definition.contains(r#"PARAMETER["central_meridian",15]"#));
        assert!(SpatialRefSys::by_id(32_701).unwrap().definition.contains(r#"PARAMETER["false_northing",10000000]"#));
        assert!(SpatialRefSys::by_id(0).is_none());
    }
}
