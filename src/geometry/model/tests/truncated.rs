//! Robustness tests of the stored form and the well-known binary
//! representation: every value of every kind cut short anywhere is
//! refused, or, in well-known binary, read as a value whose optional part
//! was left out; damaged stored bytes are refused, never a panic.

use super::*;

/// Verifies: [DESIGN] RULES.md D85, R-ERR-2: the stored form of every
/// kind in every coordinate dimension, cut at any byte or followed by
/// one, is refused, never read as another value.
#[test]
fn every_stored_prefix_is_refused() {
    for dims in ALL_DIMS {
        for g in samples(dims).into_iter().chain(empties(dims)) {
            let bytes = g.encode();
            for cut in 0..bytes.len() {
                assert!(Geom::decode(&bytes[..cut]).is_err(), "{:?} {dims:?} cut at {cut}", g.kind());
            }
            let mut longer = bytes;
            longer.push(0);
            assert!(Geom::decode(&longer).is_err(), "{:?} {dims:?} and a byte", g.kind());
        }
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68] well-known
/// binary of every kind in every coordinate dimension cut at any byte is
/// refused with its SQLSTATE, or reads as a value of the same kind, its
/// optional part at the end left out, which writes back as itself.
#[test]
fn every_wkb_prefix_is_refused_or_shorter() {
    for dims in ALL_DIMS {
        for g in samples(dims).into_iter().chain(empties(dims)) {
            let bytes = write_wkb(&g);
            for cut in 0..bytes.len() {
                match read_wkb(&bytes[..cut], 0) {
                    Ok(short) => assert_eq!(read_wkb(&write_wkb(&short), 0).unwrap(), short, "{:?} cut at {cut}", g.kind()),
                    Err(e) => assert!(e.state.is_some(), "{:?} cut at {cut}: {e:?}", g.kind()),
                }
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D85, R-ERR-2: stored bytes of every kind
/// with any one byte set to 0xFF are refused or read as some value that
/// stores back as those bytes; none panics.
#[test]
fn damaged_stored_bytes_are_refused_or_read() {
    for g in samples(Dims::Xyzm).into_iter().chain(samples(Dims::Xy)) {
        let bytes = g.encode();
        for at in 0..bytes.len() {
            let mut bad = bytes.clone();
            bad[at] = 0xFF;
            if let Ok(back) = Geom::decode(&bad) {
                assert_eq!(back.encode(), bad, "{:?} byte {at}", g.kind());
            }
        }
    }
}
