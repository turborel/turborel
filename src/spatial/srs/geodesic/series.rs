//! The series of Karney, "Algorithms for geodesics" (J. Geodesy 87,
//! 2013), carried to sixth order in the expansion parameter ε and the
//! third flattening n: A1 and C1 (eqs. 17, 18), C1' (eq. 21), A2 and C2
//! (eqs. 42, 43), A3 and C3 (eqs. 24, 25) and C4 (eq. 63). Each block of a
//! table is a polynomial's coefficients, highest degree first, then its
//! divisor, as the paper's reference implementation lays them out.

use super::super::TWO;
use super::math::polyval;

/// The order of every series.
pub(super) const ORDER: usize = 6;
/// Coefficients of C3, of C4.
pub(super) const C3_SIZE: usize = 15;
pub(super) const C4_SIZE: usize = 21;
/// The order of the series of A1 and A2 in ε², half the order.
const HALF_ORDER: usize = 3;
/// The largest power of ε in each series.
const TOP: usize = 5;

/// (1 − ε) A1 − 1 in ε².
const A1: [f64; 5] = [1.0, 4.0, 64.0, 0.0, 256.0];
/// C1[l]/ε^l in ε², l = 1 to 6.
const C1: [f64; 18] = [-1.0, 6.0, -16.0, 32.0, -9.0, 64.0, -128.0, 2048.0, 9.0, -16.0, 768.0, 3.0, -5.0, 512.0, -7.0, 1280.0, -7.0, 2048.0];
/// C1'[l]/ε^l in ε².
const C1P: [f64; 18] = [
    205.0, -432.0, 768.0, 1536.0, 4005.0, -4736.0, 3840.0, 12288.0, -225.0, 116.0, 384.0, -7173.0, 2695.0, 7680.0, 3467.0, 7680.0, 38081.0,
    61440.0,
];
/// (1 + ε) A2 − 1 in ε².
const A2: [f64; 5] = [-11.0, -28.0, -192.0, 0.0, 256.0];
/// C2[l]/ε^l in ε².
const C2: [f64; 18] = [1.0, 2.0, 16.0, 32.0, 35.0, 64.0, 384.0, 2048.0, 15.0, 80.0, 768.0, 7.0, 35.0, 512.0, 63.0, 1280.0, 77.0, 2048.0];
/// A3: coefficients of ε^5 down to ε^0, each a polynomial in n.
const A3: [f64; 18] = [-3.0, 128.0, -2.0, -3.0, 64.0, -1.0, -3.0, -1.0, 16.0, 3.0, -1.0, -2.0, 8.0, 1.0, -1.0, 2.0, 1.0, 1.0];
/// C3[l], l = 1 to 5: coefficients of ε^5 down to ε^l, polynomials in n.
const C3: [f64; 45] = [
    3.0, 128.0, 2.0, 5.0, 128.0, -1.0, 3.0, 3.0, 64.0, -1.0, 0.0, 1.0, 8.0, -1.0, 1.0, 4.0, 5.0, 256.0, 1.0, 3.0, 128.0, -3.0, -2.0, 3.0,
    64.0, 1.0, -3.0, 2.0, 32.0, 7.0, 512.0, -10.0, 9.0, 384.0, 5.0, -9.0, 5.0, 192.0, 7.0, 512.0, -14.0, 7.0, 512.0, 21.0, 2560.0,
];
/// C4[l], l = 0 to 5: coefficients of ε^5 down to ε^l, polynomials in n.
const C4: [f64; 77] = [
    97.0, 15015.0, 1088.0, 156.0, 45045.0, -224.0, -4784.0, 1573.0, 45045.0, -10656.0, 14144.0, -4576.0, -858.0, 45045.0, 64.0, 624.0,
    -4576.0, 6864.0, -3003.0, 15015.0, 100.0, 208.0, 572.0, 3432.0, -12012.0, 30030.0, 45045.0, 1.0, 9009.0, -2944.0, 468.0, 135_135.0,
    5792.0, 1040.0, -1287.0, 135_135.0, 5952.0, -11648.0, 9152.0, -2574.0, 135_135.0, -64.0, -624.0, 4576.0, -6864.0, 3003.0, 135_135.0,
    8.0, 10725.0, 1856.0, -936.0, 225_225.0, -8448.0, 4992.0, -1144.0, 225_225.0, -1440.0, 4160.0, -4576.0, 1716.0, 225_225.0, -136.0,
    63063.0, 1024.0, -208.0, 105_105.0, 3584.0, -3328.0, 1144.0, 315_315.0, -128.0, 135_135.0, -2560.0, 832.0, 405_405.0, 128.0, 99099.0,
];

/// Reads the next polynomial of `order` and its divisor from `table`,
/// evaluated at `x`.
fn next(table: &mut std::slice::Iter<'_, f64>, order: usize, x: f64) -> f64 {
    let coefficients: Vec<f64> = table.by_ref().take(order.saturating_add(1)).copied().collect();
    let divisor = table.next().copied().unwrap_or(1.0_f64);
    polyval(&coefficients, x) / divisor
}

/// A1 − 1 of ε.
pub(super) fn a1m1(eps: f64) -> f64 {
    let t = next(&mut A1.iter(), HALF_ORDER, eps * eps);
    (t + eps) / (1.0_f64 - eps)
}

/// A2 − 1 of ε.
pub(super) fn a2m1(eps: f64) -> f64 {
    let t = next(&mut A2.iter(), HALF_ORDER, eps * eps);
    (t - eps) / (1.0_f64 + eps)
}

/// The coefficients [0, c1 .. c6] of a series in ε laid out as C1.
fn sine_series(table: &[f64], eps: f64) -> [f64; ORDER + 1] {
    let mut out = [0.0_f64; ORDER + 1];
    let mut iter = table.iter();
    let mut d = eps;
    let mut l = 1_usize;
    for slot in out.iter_mut().skip(1) {
        let order = ORDER.saturating_sub(l).checked_div(2).unwrap_or_default();
        *slot = d * next(&mut iter, order, eps * eps);
        d *= eps;
        l = l.saturating_add(1);
    }
    out
}

/// C1[0 ..= 6] of ε; C1[0] is unused.
pub(super) fn c1(eps: f64) -> [f64; ORDER + 1] {
    sine_series(&C1, eps)
}

/// C1'[0 ..= 6] of ε.
pub(super) fn c1p(eps: f64) -> [f64; ORDER + 1] {
    sine_series(&C1P, eps)
}

/// C2[0 ..= 6] of ε.
pub(super) fn c2(eps: f64) -> [f64; ORDER + 1] {
    sine_series(&C2, eps)
}

/// The coefficients of A3 in ε, from ε^5 down, for third flattening `n`.
pub(super) fn a3_coefficients(n: f64) -> [f64; ORDER] {
    let mut out = [0.0_f64; ORDER];
    let mut iter = A3.iter();
    let mut j = ORDER;
    for slot in &mut out {
        j = j.saturating_sub(1);
        *slot = next(&mut iter, j.min(TOP.saturating_sub(j)), n);
    }
    out
}

/// The coefficients of C3[1 ..= 5] in ε, for `n`.
pub(super) fn c3_coefficients(n: f64) -> [f64; C3_SIZE] {
    let mut out = [0.0_f64; C3_SIZE];
    let mut iter = C3.iter();
    let mut slots = out.iter_mut();
    for l in 1..ORDER {
        for j in (l..ORDER).rev() {
            if let Some(slot) = slots.next() {
                *slot = next(&mut iter, j.min(TOP.saturating_sub(j)), n);
            }
        }
    }
    out
}

/// The coefficients of C4[0 ..= 5] in ε, for `n`.
pub(super) fn c4_coefficients(n: f64) -> [f64; C4_SIZE] {
    let mut out = [0.0_f64; C4_SIZE];
    let mut iter = C4.iter();
    let mut slots = out.iter_mut();
    for l in 0..ORDER {
        for j in (l..ORDER).rev() {
            if let Some(slot) = slots.next() {
                *slot = next(&mut iter, TOP.saturating_sub(j), n);
            }
        }
    }
    out
}

/// A3 of ε.
pub(super) fn a3(coefficients: &[f64; ORDER], eps: f64) -> f64 {
    polyval(coefficients, eps)
}

/// C3[0 ..= 5] of ε; C3[0] is unused.
pub(super) fn c3(coefficients: &[f64; C3_SIZE], eps: f64) -> [f64; ORDER] {
    let mut out = [0.0_f64; ORDER];
    let mut rest = coefficients.as_slice();
    let mut mult = 1.0_f64;
    for (l, slot) in out.iter_mut().enumerate().skip(1) {
        let count = ORDER.saturating_sub(l);
        mult *= eps;
        let (block, tail) = rest.split_at(count.min(rest.len()));
        *slot = mult * polyval(block, eps);
        rest = tail;
    }
    out
}

/// C4[0 ..= 5] of ε.
pub(super) fn c4(coefficients: &[f64; C4_SIZE], eps: f64) -> [f64; ORDER] {
    let mut out = [0.0_f64; ORDER];
    let mut rest = coefficients.as_slice();
    let mut mult = 1.0_f64;
    for (l, slot) in out.iter_mut().enumerate() {
        let count = ORDER.saturating_sub(l);
        let (block, tail) = rest.split_at(count.min(rest.len()));
        *slot = mult * polyval(block, eps);
        rest = tail;
        mult *= eps;
    }
    out
}

/// Clenshaw summation of Σ c_k sin(2kσ) (`sine`, over c[1..]) or
/// Σ c_k cos((2k+1)σ) (over c[0..]) at sin σ, cos σ.
pub(super) fn sin_cos_series(sine: bool, s: f64, c: f64, coefficients: &[f64]) -> f64 {
    let terms = if sine { coefficients.get(1..).unwrap_or_default() } else { coefficients };
    let ar = TWO * (c - s) * (c + s);
    let mut iter = terms.iter().rev();
    let (mut y0, mut y1) = (0.0_f64, 0.0_f64);
    if terms.len() % 2 == 1 {
        y0 = iter.next().copied().unwrap_or_default();
    }
    while let (Some(a), Some(b)) = (iter.next(), iter.next()) {
        y1 = ar * y0 - y1 + a;
        y0 = ar * y1 - y0 + b;
    }
    if sine { TWO * s * c * y0 } else { c * (y0 - y1) }
}
