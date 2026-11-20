//! Robustness tests of the GML representation: every value's document cut
//! short anywhere is refused with its SQLSTATE or reads as some value.

use super::*;

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.60 §5.1.61] the
/// GML of every type, empty and not, in two and three dimensions, cut at
/// any character, is refused with an SQLSTATE, or reads as a value whose
/// GML reads back as it.
#[test]
fn every_gml_prefix_is_refused_or_a_value() {
    let mut refused = 0_usize;
    for dims in [Dims::Xy, Dims::Xyz] {
        for value in values(dims) {
            let text = write(&value, Some("EPSG:4326"));
            for cut in text.char_indices().filter_map(|(at, _)| text.get(..at)) {
                match read(cut) {
                    Ok((short, _)) => assert_eq!(read(&write(&short, None)).unwrap().0.body, short.body, "{cut}"),
                    Err(e) => {
                        assert!(e.state.is_some(), "{cut}: {e:?}");
                        refused = refused.saturating_add(1);
                    }
                }
            }
        }
    }
    assert!(refused > 1000, "{refused}");
}
