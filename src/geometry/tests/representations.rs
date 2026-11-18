//! The bound of the GML representation [ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §5.1.60 DR 1, §5.1.61 DR 1].

use crate::error::SqlState;
use crate::geometry::MAX_GEOMETRY_AS_GML;
use crate::geometry::representations::gml_limited;

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.61 DR 1;
/// ISO/IEC 9075-2:2023 §9.2 GR 3b vii] GML of ST_MaxGeometryAsGML
/// characters is kept, and of one more raises string data, right
/// truncation, whichever routine writes or reads it.
#[test]
fn gml_is_bounded() {
    let most = usize::try_from(MAX_GEOMETRY_AS_GML).unwrap();
    assert_eq!(gml_limited("é".repeat(most)).unwrap().chars().count(), most);
    assert_eq!(gml_limited("x".repeat(most.saturating_add(1))).unwrap_err().sqlstate(), SqlState::STRING_DATA_RIGHT_TRUNCATION);
}
