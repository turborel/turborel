//! Unit tests of VECTOR: the image and the type here, the rest in
//! `tests/`.

mod aggregates;
mod functions;
mod kernels;
mod quantizers;
mod text;

use super::*;
use crate::bytes::Reader;
use crate::value::DataType;
use crate::value::types::TypeParams;
use random::Random;

/// A VECTOR value of `values` in `element`.
pub(super) fn dense(element: Element, values: &[f64]) -> Value {
    Vector::dense(element, values.to_vec()).unwrap().value().unwrap()
}

/// A sparse VECTOR value.
pub(super) fn sparse(element: Element, dims: u32, pairs: &[(u32, f64)]) -> Value {
    Vector::sparse(element, dims, pairs.to_vec()).unwrap().value().unwrap()
}

/// `count` seeded values in [-1, 1).
pub(super) fn noise(random: &mut Random, count: usize) -> Vec<f64> {
    (0..count).map(|_| f64::from(u32::try_from(random.next() >> 40_i32).unwrap()) / f64::from(1_u32 << 23_i32) - 1.0_f64).collect()
}

/// The VECTOR type of `params`.
pub(super) fn typed_of(params: &[u32]) -> DataType {
    DataType::Extension(ExtensionType { kind: &VECTOR_KIND, params: TypeParams::of(params).unwrap() })
}

use crate::value::types::ExtensionType;

/// Verifies: [EXTENSION] the design document's §3.2 formats: codes 1 to
/// 5, bits per element, packed sizes (§4.1 `12 + ceil(dim * bits / 8)`),
/// names in any case, and R6.9's wider format.
#[test]
fn element_formats_match_the_document() {
    let codes: Vec<u8> = Element::ALL.iter().map(|e| e.code()).collect();
    assert_eq!(codes, [1, 2, 3, 4, 5]);
    assert!(Element::ALL.iter().all(|e| Element::of_code(e.code()) == Some(*e)));
    assert_eq!(Element::of_code(0), None);
    assert_eq!(Element::of_code(6), None);
    assert_eq!(Element::Float32.packed(1536), 6144);
    assert_eq!(Element::Binary.packed(1024), 128);
    assert_eq!(Element::Binary.packed(12), 2);
    assert_eq!(Element::named("float16"), Some(Element::Float16));
    assert_eq!(Element::named("bfloat16"), None);
    assert_eq!(Element::Int8.wider(Element::Float16), Element::Float32);
    assert_eq!(Element::Float16.wider(Element::Float16), Element::Float16);
    assert_eq!(Element::Float32.wider(Element::Float64), Element::Float64);
    assert_eq!(Element::Binary.wider(Element::Binary), Element::Float32);
    assert_eq!(Layout::named("sparse"), Some(Layout::Sparse));
}

/// Verifies: [EXTENSION] the §4.1 dense image: version 1, format, flags
/// (normalized, has-norm), reserved zero, dims and the f32 norm, then the
/// packed little-endian elements; read in place by `VectorRef`.
#[test]
fn dense_image_has_the_documented_header() {
    let v = dense(Element::Float32, &[3.0_f64, 4.0_f64]);
    let bytes = image(&v).unwrap();
    assert_eq!(bytes.len(), HEADER + 8);
    assert_eq!(&bytes[..4], &[VERSION, 1, FLAG_HAS_NORM, 0]);
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2);
    assert_eq!(f32::from_le_bytes(bytes[8..12].try_into().unwrap()), 5.0);
    assert_eq!(f32::from_le_bytes(bytes[12..16].try_into().unwrap()), 3.0);
    let view = vector_of(&v).unwrap();
    assert_eq!((view.element, view.dims, view.norm, view.layout()), (Element::Float32, 2, Some(5.0), Layout::Dense));
    let unit = dense(Element::Float64, &[0.6_f64, 0.8_f64]);
    assert!(vector_of(&unit).unwrap().normalized);
    let mut out = Vec::new();
    to_f32(view, &mut out);
    assert_eq!(out, [3.0, 4.0]);
}

/// Verifies: [EXTENSION] the §4.2 sparse image: header with the sparse
/// flag, the stored count, ascending u32 indexes, then the values;
/// zeros dropped (R4.1) and unstored dimensions read as zero.
#[test]
fn sparse_image_has_the_documented_layout() {
    let v = sparse(Element::Float32, 30522, &[(17, 0.5_f64), (900, 0.0_f64), (2048, 1.25_f64)]);
    let bytes = image(&v).unwrap();
    assert_eq!(bytes[2], FLAG_SPARSE | FLAG_HAS_NORM);
    assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 2);
    assert_eq!(bytes.len(), HEADER + 4 + 2 * 4 + 2 * 4);
    let view = vector_of(&v).unwrap();
    assert_eq!((view.layout(), view.dims, view.stored()), (Layout::Sparse, 30522, 2));
    assert_eq!(view.at(17), Some(0.5_f64));
    assert_eq!(view.at(18), Some(0.0_f64));
    assert_eq!(view.at(2048), Some(1.25_f64));
    assert_eq!(view.at(30522), None);
    assert_eq!(view.nonzero(), 2);
}

/// Verifies: [EXTENSION] the binary format packs the first dimension in
/// each byte's high bit; INT8 and FLOAT16 elements round-trip.
#[test]
fn packed_formats_round_trip() {
    let bits = [1.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64, 1.0_f64];
    let packed = dense(Element::Binary, &bits);
    assert_eq!(&image(&packed).unwrap()[HEADER..], &[0b1000_0111]);
    assert_eq!(Vector::of(vector_of(&packed).unwrap()).values(), bits);
    let bytes = dense(Element::Int8, &[-128.0_f64, 0.0_f64, 127.0_f64]);
    assert_eq!(Vector::of(vector_of(&bytes).unwrap()).values(), [-128.0_f64, 0.0_f64, 127.0_f64]);
    let halves = dense(Element::Float16, &[0.5_f64, -65_504.0_f64]);
    assert_eq!(Vector::of(vector_of(&halves).unwrap()).values(), [0.5_f64, -65_504.0_f64]);
}

/// Verifies: [EXTENSION] R3.9, §3.2 and R3.4: NaN, infinity, an INT8
/// element out of range or not an integer, a FLOAT16 or FLOAT32 overflow,
/// a BINARY element not 0 or 1, and a dimension count out of range are
/// refused with SQLCODE -301, SQLSTATE 22003.
#[test]
fn elements_out_of_range_are_refused() {
    let refused = |element: Element, values: Vec<f64>| {
        let err = Vector::dense(element, values).and_then(|v| v.encode()).unwrap_err();
        assert_eq!((err.sqlcode(), err.sqlstate().as_str()), (-301_i32, "22003"), "{element:?}");
    };
    refused(Element::Float32, vec![f64::NAN]);
    refused(Element::Float64, vec![f64::INFINITY]);
    refused(Element::Int8, vec![128.0_f64]);
    refused(Element::Int8, vec![1.5_f64]);
    refused(Element::Float16, vec![70_000.0_f64]);
    refused(Element::Float32, vec![1e39_f64]);
    refused(Element::Binary, vec![2.0_f64; 8]);
    refused(Element::Binary, vec![1.0_f64; 12]);
    refused(Element::Float32, vec![]);
    refused(Element::Float32, vec![0.0_f64; 65536]);
    let err = Vector::sparse(Element::Float32, 10, vec![(3, 1.0_f64), (3, 2.0_f64)]).unwrap_err();
    assert_eq!(err.sqlstate().as_str(), "22018");
    assert!(Vector::sparse(Element::Float32, 10, vec![(10, 1.0_f64)]).is_err());
    assert!(Vector::sparse(Element::Binary, 16, vec![]).is_err());
}

/// Verifies: [DESIGN] an image that is short, of an unknown version or
/// format, or not canonical is refused with SQLCODE -916.
#[test]
fn bad_images_are_corrupt() {
    let v = dense(Element::Float32, &[1.0_f64, 2.0_f64]);
    let good = image(&v).unwrap().to_vec();
    assert!(build::canonical(&good).is_ok());
    for (at, byte) in [(0, 2), (1, 9), (2, 0x80), (3, 1), (8, 0)] {
        let mut bad = good.clone();
        bad[at] = byte;
        assert_eq!(build::canonical(&bad).unwrap_err().sqlcode(), -916_i32, "{at}");
    }
    assert_eq!(VectorRef::decode(&good[..15]).unwrap_err().sqlcode(), -916_i32);
    assert_eq!(VectorRef::decode(&[]).unwrap_err().sqlcode(), -916_i32);
}

/// Verifies: [EXTENSION] the type's parameters as written: `param_word`
/// reads `*`, formats and layouts in any case (R3.3); `pack_params` folds
/// the layout into the second, and refuses a dimension count of 0 (R3.4).
#[test]
fn parameter_words_are_read() {
    assert_eq!(pack_params(&[param_word(0, "*").unwrap()]), Some(vec![ANY]));
    assert_eq!(pack_params(&[0]), None);
    assert_eq!(param_word(0, "FLOAT32"), None);
    assert_eq!(param_word(FORMAT_PARAM, "int8"), Some(4));
    assert_eq!(param_word(FORMAT_PARAM, "*"), Some(ANY));
    assert_eq!(param_word(LAYOUT_PARAM, "Sparse"), Some(LAYOUT_STEP));
    assert_eq!(param_word(LAYOUT_PARAM, "DENSE"), Some(0));
    assert_eq!(param_word(3, "DENSE"), None);
    assert_eq!(pack_params(&[30522, 1, LAYOUT_STEP]), Some(vec![30522, 1 + LAYOUT_STEP]));
    assert_eq!(pack_params(&[1, 2, 3, 4]), None);
}

/// Verifies: [EXTENSION] the shapes and written forms follow R3.1 and
/// R3.2; bad shapes are refused with SQLCODE -103 (VEC-0001, VEC-0002);
/// the catalog's bytes of a type read back as it.
#[test]
fn parameters_encode_dims_format_and_layout() {
    let written = |p: &[u32]| VECTOR.written(&typed_of(p));
    assert_eq!(written(&[]), "VECTOR");
    assert_eq!(written(&[1536]), "VECTOR(1536)");
    assert_eq!(written(&[768, 4]), "VECTOR(768, INT8)");
    assert_eq!(written(&[0, 0]), "VECTOR(*, *)");
    assert_eq!(written(&[30522, 1 + LAYOUT_STEP]), "VECTOR(30522, FLOAT32, SPARSE)");
    assert_eq!(fixed_shape(&typed_of(&[768, 4])), Some((768, Element::Int8)));
    assert_eq!(fixed_shape(&typed_of(&[0, 4])), None);
    assert_eq!(fixed_shape(&data_type()), None);
    for bad in [vec![0, 9], vec![65536], vec![12, 5], vec![16, 5 + LAYOUT_STEP], vec![4, 2 * LAYOUT_STEP]] {
        assert_eq!(VECTOR.check_params(&bad).unwrap_err().sqlcode(), -103_i32, "{bad:?}");
    }
    assert!(VECTOR.check_params(&[100_000, 1 + LAYOUT_STEP]).is_ok());
    let t = typed(Shape { dims: Some(3), element: Some(Element::Float16), layout: Some(Layout::Dense) }).unwrap();
    let mut out = Vec::new();
    VECTOR.put_params(&t, &mut out);
    assert_eq!(VECTOR.params(&mut Reader::new(&out, "test")).unwrap(), t);
    assert_eq!(types()[0].name(), NAME);
}

/// Verifies: [EXTENSION] R5.1, R5.2, R5.3 store assignment: text is read
/// against the column's shape; a vector of another dimension count is
/// refused (SQLCODE -301, SQLSTATE 22000); formats convert among floats
/// and INT8, never to or from BINARY.
#[test]
fn assignment_converts_formats_never_dimensions() {
    let t = typed_of(&[3, 2]);
    assert!(VECTOR.assigns_text());
    let v = VECTOR.assign(Value::Char("[1, 2, 3]".into()), &t, "§9.2").unwrap();
    assert_eq!(vector_of(&v).unwrap().element, Element::Float16);
    let wide = dense(Element::Float64, &[1.0_f64, 2.0_f64, 3.0_f64]);
    let narrowed = VECTOR.assign(wide, &t, "§9.2").unwrap();
    assert_eq!(VECTOR.text(&narrowed), "[1,2,3]");
    let err = VECTOR.assign(dense(Element::Float32, &[1.0_f64, 2.0_f64]), &t, "§9.2").unwrap_err();
    assert_eq!((err.sqlcode(), err.sqlstate().as_str()), (-301_i32, "22000"));
    let short = VECTOR.assign(Value::Char("[1,2]".into()), &t, "§9.2").unwrap_err();
    assert_eq!(short.sqlcode(), -301_i32);
    let bits = typed_of(&[8, 5]);
    assert!(VECTOR.assign(dense(Element::Float32, &[1.0_f64; 8]), &bits, "§9.2").is_err());
    let open = data_type();
    let s = sparse(Element::Float32, 100_000, &[(5, 1.0_f64)]);
    assert_eq!(VECTOR.assign(s.clone(), &open, "§9.2").unwrap().kind_name(), NAME);
    assert!(VECTOR.cast(Value::Null, &t).unwrap().is_null());
    assert!(VECTOR.cast(Value::Bool(true), &t).is_err());
    let int8 = typed_of(&[2, 4]);
    assert_eq!(VECTOR.cast(dense(Element::Float32, &[1.5_f64, 2.0_f64]), &int8).unwrap_err().sqlstate().as_str(), "22003");
}

trait KindName {
    fn kind_name(&self) -> &str;
}

impl KindName for Value {
    fn kind_name(&self) -> &str {
        let Value::Extension(e) = self else { return "" };
        e.kind.name()
    }
}

/// Verifies: [DESIGN] RULES.md D53's contract for VECTOR's records. A
/// fixed column keeps a one-byte header and the elements (R4.4) and
/// reads back the same value, and a flexible one keeps the image.
#[test]
fn type_contract_round_trips() {
    let fixed = typed_of(&[3]);
    let v = dense(Element::Float32, &[0.5_f64, -1.0_f64, 2.0_f64]);
    assert_eq!(VECTOR.record_width(&fixed), Some(13));
    let record = VECTOR.encode_record(&v, &fixed).unwrap();
    assert_eq!(record.len(), 13);
    let back = VECTOR.decode_record(&record, &fixed).unwrap();
    assert_eq!(VECTOR.compare(&back, &v), Some(std::cmp::Ordering::Equal));
    let open = data_type();
    assert_eq!(VECTOR.record_width(&open), None);
    let s = sparse(Element::Float64, 50, &[(1, 2.0_f64)]);
    let image_record = VECTOR.encode_record(&s, &open).unwrap();
    assert_eq!(VECTOR.compare(&VECTOR.decode_record(&image_record, &open).unwrap(), &s), Some(std::cmp::Ordering::Equal));
    assert!(VECTOR.encode_record(&s, &fixed).is_err());
}

/// Verifies: [DESIGN] RULES.md D53's contract for VECTOR's keys, log
/// bytes, buckets, anchors and literals, each reading back or naming the
/// same value.
#[test]
fn type_contract_keys_and_literals() {
    let fixed = typed_of(&[3]);
    let open = data_type();
    let v = dense(Element::Float32, &[0.5_f64, -1.0_f64, 2.0_f64]);
    let s = sparse(Element::Float64, 50, &[(1, 2.0_f64)]);
    let mut key = Vec::new();
    VECTOR.encode_key(&v, &fixed, &mut key).unwrap();
    key.push(0xff);
    let mut rest = key.as_slice();
    let read = VECTOR.decode_key(&mut rest, &fixed).unwrap();
    assert_eq!((VECTOR.compare(&read, &v), rest), (Some(std::cmp::Ordering::Equal), &[0xff_u8][..]));
    let mut log = Vec::new();
    VECTOR.put_value(&v, &mut log);
    let logged = VECTOR.value(&mut Reader::new(&log, "test")).unwrap();
    assert_eq!(VECTOR.bucket(&logged), VECTOR.bucket(&v));
    assert_eq!(VECTOR.anchor(&v, &fixed).map(|(a, exact)| (VECTOR.compare(&a, &v), exact)), Some((Some(std::cmp::Ordering::Equal), true)));
    assert!(VECTOR.anchor(&s, &fixed).is_none());
    assert!(VECTOR.comparable(&open) && !VECTOR.comparable(&DataType::Boolean));
    assert_eq!(VECTOR.literal(&v), "TO_VECTOR('[0.5,-1,2]', 3, 'FLOAT32')");
    assert_eq!(VECTOR.literal(&s), "TO_VECTOR('[50,[1],[2]]', 50, 'FLOAT64', 'SPARSE')");
    assert_eq!(VECTOR.descriptor(&open).name, NAME);
    assert_eq!(VECTOR.position(&v), None);
}
