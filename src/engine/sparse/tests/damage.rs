//! Robustness of the posting block codec and the sparse image: blocks and
//! images cut short or damaged are refused, never misread.

use super::*;
use crate::engine::sparse::block::Quantization;
use crate::error::Code;

/// A block of rows far apart and alike, each weight distinct.
fn postings() -> Postings {
    let mut p = Postings::single(3, 0.5);
    for (row, weight) in [(3, 0.75), (900, -1.25), (1_u64 << 40_u32, 2.0), (u64::MAX - 1, 4.5)] {
        p.insert(row, weight);
    }
    p
}

/// Verifies: [DESIGN] the posting block codec, R-ERR-2: a block of every
/// quantization cut at any byte, or followed by one, is refused with
/// SQLCODE -916 as it is decoded.
#[test]
fn every_block_prefix_is_refused() {
    for quantization in [Quantization::Float32, Quantization::Float16, Quantization::Uint8] {
        let bytes = postings().encode(quantization).unwrap();
        assert_eq!(Postings::decode(&bytes).unwrap().rows, postings().rows);
        for cut in 0..bytes.len() {
            assert_eq!(Postings::decode(&bytes[..cut]).unwrap_err().code, Code::Corrupt, "{quantization:?} cut at {cut}");
        }
        let mut longer = bytes;
        longer.push(0);
        assert_eq!(Postings::decode(&longer).unwrap_err().code, Code::Corrupt);
    }
}

/// Verifies: [DESIGN] the posting block codec, R-ERR-2: a header of
/// another format or quantization, of no postings, of bounds not finite
/// or crossed, of a negative error, of row deltas wider than a row, or
/// whose last row overflows is refused with SQLCODE -916; a block with
/// any one byte set to 0xFF is refused or decodes, never a panic.
#[test]
fn damaged_headers_and_blocks_are_refused() {
    let good = postings().encode(Quantization::Float32).unwrap();
    let damaged = |at: usize, bytes: &[u8]| {
        let mut bad = good.clone();
        bad[at..at + bytes.len()].copy_from_slice(bytes);
        Header::read(&bad).map(|_| ())
    };
    for (at, bytes) in [
        (0, &[2_u8][..]),
        (1, &[9]),
        (2, &[0, 0]),
        (4, &f32::NAN.to_le_bytes()),
        (8, &f32::MAX.to_le_bytes()),
        (12, &(-1.0_f32).to_le_bytes()),
        (16, &[65]),
        (17, &[0xFF; 10]),
    ] {
        assert_eq!(damaged(at, bytes).unwrap_err().code, Code::Corrupt, "byte {at}");
    }
    let mut overflow = good[..17].to_vec();
    overflow.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
    overflow.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
    assert_eq!(Header::read(&overflow).unwrap_err().code, Code::Corrupt, "last row overflows");
    for at in 0..good.len() {
        let mut bad = good.clone();
        bad[at] = 0xFF;
        if let Err(e) = Postings::decode(&bad) {
            assert_eq!(e.code, Code::Corrupt, "byte {at}");
        }
    }
}

/// Verifies: [EXTENSION] (§4.2, R3.9) a sparse vector's image cut at any
/// byte, or followed by one, is refused with SQLCODE -301.
#[test]
fn every_image_prefix_is_refused() {
    let image = SparseVector::new(30_522, [(3, 0.25), (17, -2.0), (30_000, 7.5)]).unwrap().image();
    for cut in 0..image.len() {
        assert!(SparseVector::from_image(&image[..cut]).is_err(), "cut at {cut}");
    }
    assert!(SparseVector::from_image(&[image, vec![0]].concat()).is_err());
}
