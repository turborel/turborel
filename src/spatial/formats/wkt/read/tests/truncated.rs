//! Robustness tests of the well-known text: every value's text cut short
//! anywhere is refused with its SQLSTATE or reads as some value.

use super::*;

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.67] the text of
/// every type, empty and not, in every coordinate dimension, cut at any
/// character, is refused with an SQLSTATE, or, where the cut leaves a
/// text of its own (a number cut short), reads as a value whose canonical
/// text reads back as it.
#[test]
fn every_text_prefix_is_refused_or_a_value() {
    let mut refused = 0_usize;
    for dims in DIMS {
        for value in values(dims) {
            let text = write(&value);
            for cut in text.char_indices().filter_map(|(at, _)| text.get(..at)) {
                match read(cut) {
                    Ok(short) => assert_eq!(read(&write(&short)).unwrap(), short, "{cut}"),
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
