//! The sparse image of §4.2 and the posting block codec: round trips,
//! refusals, and the error bound a block keeps.

use super::*;
use crate::engine::sparse::vector::{FORMAT_BINARY, FORMAT_FLOAT16, FORMAT_FLOAT64, FORMAT_INT8, IMAGE_VERSION, SPARSE_FLAG};
use crate::error::Code;

/// A §4.2 image of `format` with `dims`, positions and element bytes.
fn image(format: u8, dims: u32, positions: &[u32], elements: &[u8]) -> Vec<u8> {
    let mut out = vec![IMAGE_VERSION, format, SPARSE_FLAG, 0];
    out.extend_from_slice(&dims.to_le_bytes());
    out.extend_from_slice(&0_f32.to_le_bytes());
    out.extend_from_slice(&u32::try_from(positions.len()).unwrap().to_le_bytes());
    for p in positions {
        out.extend_from_slice(&p.to_le_bytes());
    }
    out.extend_from_slice(elements);
    out
}

/// Verifies: [EXTENSION] (§4.2) A sparse vector's FLOAT32 image round
/// trips; FLOAT16, FLOAT64 and INT8 images read to the same values; an
/// explicit zero is dropped (R4.1).
#[test]
fn sparse_images_round_trip_in_every_format() {
    let v = SparseVector::new(30_522, [(3, 0.25), (17, -2.0), (30_000, 7.5)]).unwrap();
    assert_eq!(SparseVector::from_image(&v.image()).unwrap(), v);
    let halves: Vec<u8> = [0.25_f32, -2.0, 7.5].iter().flat_map(|&x| half::to_half(x).to_le_bytes()).collect();
    assert_eq!(SparseVector::from_image(&image(FORMAT_FLOAT16, 30_522, &[3, 17, 30_000], &halves)).unwrap(), v);
    let doubles: Vec<u8> = [0.25_f64, -2.0_f64, 7.5_f64].iter().flat_map(|x| x.to_le_bytes()).collect();
    assert_eq!(SparseVector::from_image(&image(FORMAT_FLOAT64, 30_522, &[3, 17, 30_000], &doubles)).unwrap(), v);
    let bytes = SparseVector::from_image(&image(FORMAT_INT8, 10, &[1, 2, 9], &[5, 0, 0x80])).unwrap();
    assert_eq!(bytes.indices(), [1, 9]);
    assert_eq!(bytes.values(), [5.0, -128.0]);
}

/// Verifies: [EXTENSION] (R3.4, R3.8, R3.9, VEC-0016) Positions not
/// ascending, repeated or past the dimensions, NaN or infinite values,
/// a BINARY or dense image, a dimension count out of range and an image
/// of the wrong length are refused with SQLCODE -301.
#[test]
fn malformed_sparse_vectors_are_refused() {
    let refused = |r: Result<SparseVector>| assert_eq!(r.unwrap_err().code, Code::Data);
    refused(SparseVector::new(10, [(3, 1.0), (3, 2.0)]));
    refused(SparseVector::new(10, [(4, 1.0), (3, 2.0)]));
    refused(SparseVector::new(10, [(10, 1.0)]));
    refused(SparseVector::new(10, [(1, f32::NAN)]));
    refused(SparseVector::new(10, [(1, f32::INFINITY)]));
    refused(SparseVector::new(0, []));
    refused(SparseVector::new(0x8000_0000, []));
    refused(SparseVector::from_image(&image(FORMAT_BINARY, 16, &[1], &[1])));
    let mut dense = image(FORMAT_INT8, 10, &[1], &[1]);
    dense[2] = 0;
    refused(SparseVector::from_image(&dense));
    let mut short = image(FORMAT_INT8, 10, &[1, 2], &[1, 2]);
    short.pop();
    refused(SparseVector::from_image(&short));
    refused(SparseVector::from_image(&image(FORMAT_FLOAT64, 10, &[1], &f64::NAN.to_le_bytes())));
}

/// Verifies: [DESIGN] Equal vectors hash alike and differing ones differ,
/// in a value, a position or the dimensions; a dot product sums only the
/// positions both hold.
#[test]
fn hashes_and_dot_products() {
    let a = SparseVector::new(100, [(1, 1.0), (5, 2.0)]).unwrap();
    assert_eq!(a.hash(), SparseVector::new(100, [(1, 1.0), (5, 2.0)]).unwrap().hash());
    assert_ne!(a.hash(), SparseVector::new(100, [(1, 1.0), (5, 2.5)]).unwrap().hash());
    assert_ne!(a.hash(), SparseVector::new(100, [(1, 1.0), (6, 2.0)]).unwrap().hash());
    assert_ne!(a.hash(), SparseVector::new(101, [(1, 1.0), (5, 2.0)]).unwrap().hash());
    let b = SparseVector::new(100, [(0, 9.0), (5, 3.0), (99, 1.0)]).unwrap();
    assert_eq!(a.dot(&b).to_bits(), 6.0_f64.to_bits());
}

/// Verifies: [DESIGN] For every quantization, a block decodes to its rows
/// exactly, its stored weights each within its header's error of the
/// weight inserted, its bounds holding every weight inserted; rows far
/// apart, near 2^64, and repeated rows round trip through bit packing.
#[test]
fn blocks_keep_rows_exactly_and_weights_within_their_error() {
    let mut random = Random(89);
    let mut rows: Vec<u64> = (0_u32..300).map(|_| random.below(1 << 40)).collect();
    rows.extend([u64::MAX - 1, u64::MAX, 0, 0]);
    rows.sort_unstable();
    let weights: Vec<f32> = rows.iter().map(|_| (random.unit() - 0.3) * 50.0).collect();
    for quantization in [Quantization::Float32, Quantization::Float16, Quantization::Uint8] {
        let mut p = Postings::single(rows[0], weights[0]);
        for (&r, &w) in rows.iter().zip(&weights).skip(1) {
            p.insert(r, w);
        }
        let bytes = p.encode(quantization).unwrap();
        let header = Header::read(&bytes).unwrap();
        let back = Postings::decode(&bytes).unwrap();
        assert_eq!(back.rows, rows);
        assert_eq!((header.count, header.last), (rows.len(), u64::MAX));
        for (&w, &stored) in weights.iter().zip(&back.weights) {
            assert!((f64::from(w) - stored).abs() <= f64::from(header.error), "{quantization:?} {w} {stored} {}", header.error);
            assert!(header.min <= w && w <= header.max);
        }
        let again = Postings::decode(&back.encode(quantization).unwrap()).unwrap();
        assert_eq!(again.weights, back.weights, "{quantization:?}: a block stored again keeps its codes");
    }
}

/// Verifies: [DESIGN] A block whose bytes are damaged is refused with
/// SQLCODE -916, never misread: a wrong format, a count of 0, a cut
/// payload, or a last row not its rows'.
#[test]
fn damaged_blocks_are_refused() {
    let mut p = Postings::single(10, 1.0);
    p.insert(20, 2.0);
    let good = p.encode(Quantization::Uint8).unwrap();
    let corrupt = |bytes: Vec<u8>| assert_eq!(Postings::decode(&bytes).unwrap_err().code, Code::Corrupt);
    let mut format = good.clone();
    format[0] = 9;
    corrupt(format);
    let mut count = good.clone();
    count[2] = 0;
    count[3] = 0;
    corrupt(count);
    let mut cut = good.clone();
    cut.pop();
    corrupt(cut);
    let mut last = good;
    assert_eq!(last[18], 10, "the last row less the first");
    last[18] = 11;
    corrupt(last);
}
