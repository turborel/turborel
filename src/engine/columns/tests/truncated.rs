//! Robustness tests of column chunks: a chunk of each encoding cut short
//! anywhere is refused, never read as other values. Fixed and varying
//! chunks are written uncompressed, the others compressed.

use super::*;

/// One chunk of each encoding, its type, and the encoding's tag.
fn samples() -> Vec<(DataType, Vec<Value>, u8)> {
    let texts = |n: u32, f: &dyn Fn(u32) -> String| -> Vec<Value> { (0..n).map(|k| Value::Char(f(k).into())).collect() };
    let mut all = vec![
        (INTEGER, (-3..9000).step_by(1000).enumerate().map(|(k, n)| if k % 4 == 1 { Value::Null } else { int(n) }).collect(), FIXED),
        (TEXT, texts(6, &|k| "x".repeat(usize::try_from(k).unwrap())), VARYING),
        (INTEGER, vec![int(42); 20], CONSTANT),
        (INTEGER, (1_000_000..1_000_200).step_by(2).map(int).chain([Value::Null]).collect(), FRAME),
        (TEXT, texts(300, &|k| ["red", "green", "blue"][usize::try_from(k % 3).unwrap()].to_owned()), DICTIONARY),
        (INTEGER, (0..2000_i128).map(|n| int(n.checked_div(250).unwrap())).collect(), RUNS),
        (TEXT, texts(300, &|k| format!("customer{k}@mail{}.example.com", k % 7)), FSST),
    ];
    let prices = (0_i32..1000_i32).map(|n| Value::Approx(f64::from(n.checked_mul(37).unwrap().checked_rem(10_000).unwrap()) / 100.0));
    all.push((DOUBLE, prices.chain([Value::Approx(1.0 / 3.0)]).collect(), ALP));
    all
}

/// Whether the chunk `bytes` is refused, as it is parsed or as one of its
/// `count` values of type `t` is read, with SQLCODE -916.
fn refused(bytes: &[u8], t: &DataType, count: usize) -> bool {
    match Chunk::parse(bytes) {
        Err(e) => e.code == Code::Corrupt,
        Ok(chunk) => (0..count).any(|n| chunk.value(n, t).is_err_and(|e| e.code == Code::Corrupt)),
    }
}

/// Verifies: [DESIGN] RULES.md D31, column chunks, R-ERR-2: a chunk of
/// every encoding (fixed, varying, constant, frame of reference,
/// dictionary, runs, FSST and ALP) cut at any byte is refused with
/// SQLCODE -916 when parsed or read, never a panic nor other values.
#[test]
fn every_chunk_prefix_is_refused() {
    for (t, values, kind) in samples() {
        let fields = Fields::of(&t, values.iter().cloned()).unwrap();
        let compress = kind != FIXED && kind != VARYING;
        let bytes = fields.chunk(&t, 0, (fields.len(), compress)).unwrap();
        assert_eq!(bytes[0], kind, "{values:?}");
        assert!(!refused(&bytes, &t, values.len()), "the whole chunk reads");
        for cut in 0..bytes.len() {
            assert!(refused(&bytes[..cut], &t, values.len()), "kind {kind} cut at {cut} of {}", bytes.len());
        }
    }
}
