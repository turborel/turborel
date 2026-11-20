//! Robustness of the VECTOR image: dense and sparse images of every
//! element cut short or followed by a byte are refused, never misread.

use super::*;
use crate::vector::build::canonical;

/// A dense and a sparse image of each element.
fn images() -> Vec<Vec<u8>> {
    let mut all = Vec::new();
    for element in [Element::Float32, Element::Float16, Element::Float64, Element::Int8, Element::Binary] {
        all.push(Vector::dense(element, [1.0_f64, 0.0_f64, 1.0_f64, 1.0_f64].repeat(4)).unwrap().encode().unwrap());
        if element != Element::Binary {
            all.push(Vector::sparse(element, 30, vec![(2_u32, 1.0_f64), (17_u32, -2.0_f64), (29_u32, 3.0_f64)]).unwrap().encode().unwrap());
        }
    }
    all
}

/// Verifies: [DESIGN] the VECTOR image (design document §3), R-ERR-2: a
/// dense and a sparse image of every element reads back canonical; cut
/// at any byte, or followed by one, it is refused with SQLCODE -916.
#[test]
fn every_image_prefix_is_refused() {
    for image in images() {
        let v = canonical(&image).unwrap();
        assert_eq!(Vector::of(v).encode().unwrap(), image);
        for cut in 0..image.len() {
            assert!(canonical(&image[..cut]).is_err(), "{image:?} cut at {cut}");
            if let Err(e) = VectorRef::decode(&image[..cut]) {
                assert_eq!(e.sqlcode(), -916_i32, "{image:?} cut at {cut}");
            }
        }
        assert!(canonical(&[image.as_slice(), &[0]].concat()).is_err());
    }
}
