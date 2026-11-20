//! Robustness tests of the linear referencing representations: every
//! proper prefix of a value's well-known text [ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §15.14], its GML [§15.1.12 and its peers] and its stored
//! form is refused with its exception, or reads as a value of its own,
//! never a panic.

use super::formats::samples;
use super::*;

/// The proper prefixes of `text`, each ending on a character boundary.
fn prefixes(text: &str) -> impl Iterator<Item = &str> {
    text.char_indices().filter_map(|(at, _)| text.get(..at))
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.6.9 Description
/// 2a] text cut short anywhere is not producible in the BNF and raises
/// 2FF22 or its constructor's exception, unless the prefix is itself a
/// value's text (a number cut short is still a number), which then reads
/// back from its own text.
#[test]
fn every_text_prefix_is_refused_or_a_value() {
    let all = units();
    let cat = Catalog::of_units(&all);
    let mut refused = 0_usize;
    for v in samples() {
        let text = as_text(&v).unwrap();
        for cut in prefixes(&text) {
            match from_text(cut, v.kind(), &cat) {
                Ok(back) => assert_eq!(from_text(&as_text(&back).unwrap(), v.kind(), &cat).unwrap(), back, "{cut}"),
                Err(e) => {
                    assert!(e.state.is_some(), "{cut}: {e:?}");
                    refused = refused.saturating_add(1);
                }
            }
        }
    }
    assert!(refused > 1000, "{refused}");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.12 and its
/// peers] a document cut short anywhere is not the representation and
/// raises 2FF24, or reads as a value of its own.
#[test]
fn every_gml_prefix_is_refused_or_a_value() {
    let all = units();
    let cat = Catalog::of_units(&all);
    let mut refused = 0_usize;
    for v in samples() {
        let text = as_gml(&v, &NoSrs).unwrap();
        for cut in prefixes(&text) {
            match from_gml(cut, v.kind(), &cat, &NoSrs) {
                Ok(back) => assert_eq!(from_gml(&as_gml(&back, &NoSrs).unwrap(), v.kind(), &cat, &NoSrs).unwrap(), back, "{cut}"),
                Err(e) => {
                    assert_eq!(e.state, Some(SqlState::SPATIAL_INVALID_GML), "{cut}: {e:?}");
                    refused = refused.saturating_add(1);
                }
            }
        }
    }
    assert!(refused > 1000, "{refused}");
}

/// Verifies: [DESIGN] every value's stored form cut short anywhere, or
/// followed by an octet, is corruption (SQLCODE -916).
#[test]
fn every_stored_prefix_is_corrupt() {
    for v in samples() {
        let bytes = v.encode().unwrap();
        for cut in 0..bytes.len() {
            assert_eq!(LrValue::decode(&bytes[..cut]).unwrap_err().code, Code::Corrupt, "{v:?} cut at {cut}");
        }
        let mut longer = bytes;
        longer.push(0);
        assert_eq!(LrValue::decode(&longer).unwrap_err().code, Code::Corrupt, "{v:?}");
    }
}
