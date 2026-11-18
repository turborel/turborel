//! The inverse geodesic problem, Karney 2013 §3 to §5: reduce to a
//! canonical configuration, solve meridional and equatorial geodesics
//! directly, otherwise start from the auxiliary sphere or the astroid
//! (§5) and solve for the azimuth by Newton's method on λ12, safeguarded
//! by bisection. Only oblate and spherical ellipsoids occur, as the
//! \<inverse flattening\> is zero or above one.

use std::f64::consts::{FRAC_1_SQRT_2, PI};

use super::super::{HALF, TWO};
use super::math::{self, DEGREE, HALF_TURN, norm, sincosd, sincosd_error};
use super::series::{self, sin_cos_series};
use super::{Geodesic, Inverse};

/// Iterations of Newton's method before bisection takes over, and in all.
const NEWTON: u32 = 20;
const ITERATIONS: u32 = 83;
/// Tolerances of Karney's reference implementation.
const TOL1_SCALE: f64 = 200.0;
const XTHRESH_SCALE: f64 = 1000.0;
const TRIP_SCALE: f64 = 8.0;
const NEWTON_TRIP: f64 = 16.0;
const TINY_SCALE: f64 = 3.0;
const SHORT: f64 = 0.5;
const NEAR_ANTIPODE: f64 = 6.0;
const SPHERE_N: f64 = 0.1;
/// cos 135° and the bound on sin β2 − sin β1 of the area's fast formula.
const AREA_COS: f64 = -FRAC_1_SQRT_2;
const AREA_SPAN: f64 = 1.75;

fn tiny() -> f64 {
    f64::MIN_POSITIVE.sqrt()
}

fn tol2() -> f64 {
    f64::EPSILON.sqrt()
}

/// sin β, cos β of a reduced latitude, and √(1 + e'² sin² β).
#[derive(Clone, Copy)]
pub(super) struct Reduced {
    pub(super) s: f64,
    pub(super) c: f64,
    pub(super) dn: f64,
}

pub(super) fn reduced(g: &Geodesic, lat: f64) -> Reduced {
    let (s, c) = sincosd(math::round(lat));
    let (s, c) = norm(s * g.f1, c);
    let c = c.max(tiny());
    Reduced { s, c, dn: (1.0_f64 + g.ep2 * s * s).sqrt() }
}

/// sin σ, cos σ at both ends.
#[derive(Clone, Copy)]
struct Sigmas {
    s1: f64,
    c1: f64,
    s2: f64,
    c2: f64,
}

/// The distances Lengths computes, in units of b.
struct Lengths {
    s12b: f64,
    m12b: f64,
    big_m12: f64,
    big_m21: f64,
}

/// Distance, reduced length and geodesic scales (Karney 2013 eqs. 38-41).
fn lengths(g: &Geodesic, eps: f64, sig12: f64, sig: Sigmas, ends: (Reduced, Reduced)) -> Lengths {
    let (p1, p2) = ends;
    let c1 = series::c1(eps);
    let c2 = series::c2(eps);
    let a1 = 1.0_f64 + series::a1m1(eps);
    let a2m1 = series::a2m1(eps);
    let m0 = a1 - 1.0_f64 - a2m1;
    let a2 = 1.0_f64 + a2m1;
    let b1 = sin_cos_series(true, sig.s2, sig.c2, &c1) - sin_cos_series(true, sig.s1, sig.c1, &c1);
    let b2 = sin_cos_series(true, sig.s2, sig.c2, &c2) - sin_cos_series(true, sig.s1, sig.c1, &c2);
    let j12 = m0 * sig12 + (a1 * b1 - a2 * b2);
    let m12b = p2.dn * (sig.c1 * sig.s2) - p1.dn * (sig.s1 * sig.c2) - sig.c1 * sig.c2 * j12;
    let csig12 = sig.c1 * sig.c2 + sig.s1 * sig.s2;
    let t = g.ep2 * (p1.c - p2.c) * (p1.c + p2.c) / (p1.dn + p2.dn);
    Lengths {
        s12b: a1 * (sig12 + b1),
        m12b,
        big_m12: csig12 + (t * sig.s2 - sig.c2 * j12) * sig.s1 / p1.dn,
        big_m21: csig12 - (t * sig.s1 - sig.c1 * j12) * sig.s2 / p2.dn,
    }
}

/// k² = cos² α0 e'² as ε (Karney 2013 eq. 16).
fn epsilon(g: &Geodesic, calp0: f64) -> f64 {
    let k2 = calp0 * calp0 * g.ep2;
    k2 / (TWO * (1.0_f64 + (1.0_f64 + k2).sqrt()) + k2)
}

/// A solution before the signs of the canonical configuration are undone.
struct Solution {
    sig12: f64,
    s12: f64,
    m12: f64,
    big_m12: f64,
    big_m21: f64,
    alp1: (f64, f64),
    alp2: (f64, f64),
    /// sin ω12, cos ω12 for the area, none on a meridian.
    omega: Option<(f64, f64)>,
}

/// The values of λ12 at azimuth α1, Karney 2013 §4.
struct Lambda {
    lam12: f64,
    alp2: (f64, f64),
    sig12: f64,
    sig: Sigmas,
    eps: f64,
    domg12: f64,
    dlam12: f64,
}

fn lambda12(g: &Geodesic, ends: (Reduced, Reduced), alp1: (f64, f64), lam120: (f64, f64), diffp: bool) -> Lambda {
    let (p1, p2) = ends;
    let (salp1, mut calp1) = alp1;
    if p1.s == 0.0_f64 && calp1 == 0.0_f64 {
        calp1 = -tiny();
    }
    let salp0 = salp1 * p1.c;
    let calp0 = calp1.hypot(salp1 * p1.s);
    let (ssig1, csig1) = norm(p1.s, calp1 * p1.c);
    let (somg1, comg1) = (salp0 * p1.s, calp1 * p1.c);
    let differ = p2.c.to_bits() != p1.c.to_bits();
    let salp2 = if differ { salp0 / p2.c } else { salp1 };
    let calp2 = if differ || p2.s.abs() + p1.s != 0.0_f64 {
        let shift = if p1.c < -p1.s { (p2.c - p1.c) * (p1.c + p2.c) } else { (p1.s - p2.s) * (p1.s + p2.s) };
        ((calp1 * p1.c).powi(2) + shift).sqrt() / p2.c
    } else {
        calp1.abs()
    };
    let (ssig2, csig2) = norm(p2.s, calp2 * p2.c);
    let (somg2, comg2) = (salp0 * p2.s, calp2 * p2.c);
    let sig12 = ((csig1 * ssig2 - ssig1 * csig2).max(0.0_f64) + 0.0_f64).atan2(csig1 * csig2 + ssig1 * ssig2);
    let sin_omega = (comg1 * somg2 - somg1 * comg2).max(0.0_f64) + 0.0_f64;
    let cos_omega = comg1 * comg2 + somg1 * somg2;
    let (slam120, clam120) = lam120;
    let eta = (sin_omega * clam120 - cos_omega * slam120).atan2(cos_omega * clam120 + sin_omega * slam120);
    let eps = epsilon(g, calp0);
    let c3 = series::c3(&g.c3x, eps);
    let b312 = sin_cos_series(true, ssig2, csig2, &c3) - sin_cos_series(true, ssig1, csig1, &c3);
    let domg12 = -g.f * series::a3(&g.a3x, eps) * salp0 * (sig12 + b312);
    let sig = Sigmas { s1: ssig1, c1: csig1, s2: ssig2, c2: csig2 };
    let dlam12 = if !diffp {
        0.0_f64
    } else if calp2 == 0.0_f64 {
        -TWO * g.f1 * p1.dn / p1.s
    } else {
        lengths(g, eps, sig12, sig, ends).m12b * g.f1 / (calp2 * p2.c)
    };
    Lambda { lam12: eta + domg12, alp2: (salp2, calp2), sig12, sig, eps, domg12, dlam12 }
}

/// The solution k of the astroid problem, Karney 2013 eq. 55.
fn astroid(x: f64, y: f64) -> f64 {
    let xx = x * x;
    let yy = y * y;
    let rr = (xx + yy - 1.0_f64) / NEAR_ANTIPODE;
    if yy == 0.0_f64 && rr <= 0.0_f64 {
        return 0.0_f64;
    }
    let ss = xx * yy / (TWO * TWO);
    let rr2 = rr * rr;
    let rr3 = rr * rr2;
    let disc = ss * (ss + TWO * rr3);
    let uu = if disc >= 0.0_f64 {
        let cube = ss + rr3;
        let cube = cube + if cube < 0.0_f64 { -disc.sqrt() } else { disc.sqrt() };
        let root = cube.cbrt();
        rr + root + if root == 0.0_f64 { 0.0_f64 } else { rr2 / root }
    } else {
        let angle = (-disc).sqrt().atan2(-(ss + rr3));
        rr + TWO * rr * (angle / (1.0_f64 + TWO)).cos()
    };
    let vv = (uu * uu + yy).sqrt();
    let uv = if uu < 0.0_f64 { yy / (vv - uu) } else { uu + vv };
    let ww = (uv - yy) / (TWO * vv);
    uv / ((uv + ww * ww).sqrt() + ww)
}

/// The first guess of α1, Karney 2013 §5, or a short line's solution.
struct Start {
    sig12: f64,
    alp1: (f64, f64),
    alp2: (f64, f64),
    dnm: f64,
}

fn start(g: &Geodesic, ends: (Reduced, Reduced), lam12: f64, lam: (f64, f64)) -> Start {
    let (p1, p2) = ends;
    let sbet12 = p2.s * p1.c - p2.c * p1.s;
    let cbet12 = p2.c * p1.c + p2.s * p1.s;
    let sbet12a = p2.s * p1.c + p2.c * p1.s;
    let shortline = cbet12 >= 0.0_f64 && sbet12 < SHORT && p2.c * lam12 < SHORT;
    let mut dnm = 0.0_f64;
    let (somg12, comg12) = if shortline {
        let mid = (p1.s + p2.s).powi(2);
        let mid = mid / (mid + (p1.c + p2.c).powi(2));
        dnm = (1.0_f64 + g.ep2 * mid).sqrt();
        (lam12 / (g.f1 * dnm)).sin_cos()
    } else {
        lam
    };
    let salp1 = p2.c * somg12;
    let calp1 = if comg12 >= 0.0_f64 {
        sbet12 + p2.c * p1.s * somg12 * somg12 / (1.0_f64 + comg12)
    } else {
        sbet12a - p2.c * p1.s * somg12 * somg12 / (1.0_f64 - comg12)
    };
    let ssig12 = salp1.hypot(calp1);
    let csig12 = p1.s * p2.s + p1.c * p2.c * comg12;
    if shortline && ssig12 < g.etol2 {
        let tail = if comg12 >= 0.0_f64 { somg12 * somg12 / (1.0_f64 + comg12) } else { 1.0_f64 - comg12 };
        let alp2 = norm(p1.c * somg12, sbet12 - p1.c * p2.s * tail);
        return Start { sig12: ssig12.atan2(csig12), alp1: finish(salp1, calp1), alp2, dnm };
    }
    let spherical = g.n.abs() > SPHERE_N || csig12 >= 0.0_f64 || ssig12 >= NEAR_ANTIPODE * g.n.abs() * PI * p1.c * p1.c;
    let alp1 = if spherical { finish(salp1, calp1) } else { antipodal(g, ends, lam, sbet12a) };
    Start { sig12: -1.0_f64, alp1, alp2: (0.0_f64, 0.0_f64), dnm }
}

/// α1 near the antipode from the astroid (Karney 2013 §5, oblate case).
fn antipodal(g: &Geodesic, ends: (Reduced, Reduced), lam: (f64, f64), sbet12a: f64) -> (f64, f64) {
    let (p1, p2) = ends;
    let (slam12, clam12) = lam;
    let lam12x = (-slam12).atan2(-clam12);
    let eps = epsilon(g, p1.s);
    let lamscale = g.f * p1.c * series::a3(&g.a3x, eps) * PI;
    let betscale = lamscale * p1.c;
    let x = lam12x / lamscale;
    let y = sbet12a / betscale;
    let tol1 = TOL1_SCALE * f64::EPSILON;
    if y > -tol1 && x > -1.0_f64 - XTHRESH_SCALE * tol2() {
        let salp1 = (-x).min(1.0_f64);
        return finish(salp1, -(1.0_f64 - salp1 * salp1).sqrt());
    }
    let k = astroid(x, y);
    let omg12a = lamscale * (-x * k / (1.0_f64 + k));
    let (somg12, comg12) = (omg12a.sin(), -omg12a.cos());
    finish(p2.c * somg12, sbet12a - p2.c * p1.s * somg12 * somg12 / (1.0_f64 - comg12))
}

fn finish(salp1: f64, calp1: f64) -> (f64, f64) {
    if salp1 > 0.0_f64 { norm(salp1, calp1) } else { (1.0_f64, 0.0_f64) }
}

/// Solves for α1 by Newton's method with a bisection bracket.
fn iterate(g: &Geodesic, ends: (Reduced, Reduced), first: (f64, f64), lam: (f64, f64)) -> Solution {
    let tol0 = f64::EPSILON;
    let (mut salp1, mut calp1) = first;
    let (mut a, mut b) = ((tiny(), 1.0_f64), (tiny(), -1.0_f64));
    let (mut newton_trip, mut bisection_trip) = (false, false);
    let mut numit = 0_u32;
    let found = loop {
        let v = lambda12(g, ends, (salp1, calp1), lam, numit < NEWTON);
        let error = v.lam12;
        let bound = if newton_trip { TRIP_SCALE } else { 1.0_f64 } * tol0;
        if bisection_trip || error.abs() < bound || error.is_nan() || numit >= ITERATIONS {
            break v;
        }
        if error > 0.0_f64 && (numit > NEWTON || calp1 / salp1 > b.1 / b.0) {
            b = (salp1, calp1);
        } else if error < 0.0_f64 && (numit > NEWTON || calp1 / salp1 < a.1 / a.0) {
            a = (salp1, calp1);
        }
        numit = numit.saturating_add(1);
        if let Some(next) = newton(numit <= NEWTON, error, v.dlam12, (salp1, calp1)) {
            (salp1, calp1) = next;
            newton_trip = error.abs() <= NEWTON_TRIP * tol0;
            continue;
        }
        (salp1, calp1) = norm((a.0 + b.0) * HALF, (a.1 + b.1) * HALF);
        newton_trip = false;
        let bracket = tol0 * tol2();
        bisection_trip = (a.0 - salp1).abs() + (a.1 - calp1) < bracket || (salp1 - b.0).abs() + (calp1 - b.1) < bracket;
    };
    let l = lengths(g, found.eps, found.sig12, found.sig, ends);
    let (sd, cd) = found.domg12.sin_cos();
    let (slam, clam) = lam;
    Solution {
        sig12: found.sig12,
        s12: l.s12b * g.b,
        m12: l.m12b * g.b,
        big_m12: l.big_m12,
        big_m21: l.big_m21,
        alp1: (salp1, calp1),
        alp2: found.alp2,
        omega: Some((slam * cd - clam * sd, clam * cd + slam * sd)),
    }
}

/// A Newton step of α1, if it stays in (0, π) of azimuth.
fn newton(allowed: bool, error: f64, derivative: f64, alp1: (f64, f64)) -> Option<(f64, f64)> {
    if !(allowed && derivative > 0.0_f64) {
        return None;
    }
    let dalp1 = -error / derivative;
    if dalp1.abs() >= PI {
        return None;
    }
    let (sd, cd) = dalp1.sin_cos();
    let (salp1, calp1) = alp1;
    let nsalp1 = salp1 * cd + calp1 * sd;
    (nsalp1 > 0.0_f64).then(|| norm(nsalp1, calp1 * cd - salp1 * sd))
}

/// A geodesic along a meridian, if it is the shortest (Karney 2013 §3).
fn meridian(g: &Geodesic, ends: (Reduced, Reduced), lam: (f64, f64)) -> Option<Solution> {
    let (p1, p2) = ends;
    let (slam12, clam12) = lam;
    let sig = Sigmas { s1: p1.s, c1: clam12 * p1.c, s2: p2.s, c2: p2.c };
    let mut sig12 = ((sig.c1 * sig.s2 - sig.s1 * sig.c2).max(0.0_f64) + 0.0_f64).atan2(sig.c1 * sig.c2 + sig.s1 * sig.s2);
    let l = lengths(g, g.n, sig12, sig, ends);
    let (mut s12x, mut m12x) = (l.s12b, l.m12b);
    if !(sig12 < 1.0_f64 || m12x >= 0.0_f64) {
        return None;
    }
    if sig12 < TINY_SCALE * tiny() || (sig12 < f64::EPSILON && (s12x < 0.0_f64 || m12x < 0.0_f64)) {
        (sig12, m12x, s12x) = (0.0_f64, 0.0_f64, 0.0_f64);
    }
    Some(Solution {
        sig12,
        s12: s12x * g.b,
        m12: m12x * g.b,
        big_m12: l.big_m12,
        big_m21: l.big_m21,
        alp1: (slam12, clam12),
        alp2: (0.0_f64, 1.0_f64),
        omega: None,
    })
}

/// The canonical configuration (Karney 2013 §3): |lat1| ≥ |lat2|,
/// lat1 ≤ 0, 0 ≤ λ12 ≤ 180°, and the signs that undo it.
struct Canonical {
    ends: (Reduced, Reduced),
    lon12: f64,
    lon12s: f64,
    lam: (f64, f64),
    lat1: f64,
    swapp: f64,
    lonsign: f64,
    latsign: f64,
}

fn canonical(g: &Geodesic, first: [f64; 2], second: [f64; 2]) -> Canonical {
    let ([lat1, from_lon], [lat2, to_lon]) = (first, second);
    let (mut lat1, mut lat2) = (math::latitude(lat1), math::latitude(lat2));
    let (lon12, lon12s) = math::difference(from_lon, to_lon);
    let mut lonsign = if lon12.is_sign_negative() { -1.0_f64 } else { 1.0_f64 };
    let (lon12, lon12s) = (lon12 * lonsign, lon12s * lonsign);
    let lam = sincosd_error(lon12, lon12s);
    let lon12s = (HALF_TURN - lon12) - lon12s;
    let swapp = if lat1.abs() < lat2.abs() || lat2.is_nan() { -1.0_f64 } else { 1.0_f64 };
    if swapp < 0.0_f64 {
        lonsign = -lonsign;
        std::mem::swap(&mut lat1, &mut lat2);
    }
    let latsign = if lat1.is_sign_negative() { 1.0_f64 } else { -1.0_f64 };
    let (lat1, lat2) = (lat1 * latsign, lat2 * latsign);
    let mut p1 = reduced(g, lat1);
    let mut p2 = reduced(g, lat2);
    if p1.c < -p1.s {
        if p2.c.to_bits() == p1.c.to_bits() {
            p2.s = p1.s.copysign(p2.s);
        }
    } else if p2.s.abs() + p1.s == 0.0_f64 {
        p2.c = p1.c;
    }
    p1.dn = (1.0_f64 + g.ep2 * p1.s * p1.s).sqrt();
    p2.dn = (1.0_f64 + g.ep2 * p2.s * p2.s).sqrt();
    Canonical { ends: (p1, p2), lon12, lon12s, lam, lat1, swapp, lonsign, latsign }
}

/// Solves the canonical problem.
fn solve(g: &Geodesic, k: &Canonical) -> Solution {
    let lam12 = k.lon12 * DEGREE;
    let on_meridian = k.lat1 == -math::QUARTER || k.lam.0 == 0.0_f64;
    if let Some(solution) = on_meridian.then(|| meridian(g, k.ends, k.lam)).flatten() {
        return solution;
    }
    let (p1, _) = k.ends;
    if p1.s == 0.0_f64 && (g.f <= 0.0_f64 || k.lon12s >= g.f * HALF_TURN) {
        let sig12 = lam12 / g.f1;
        return Solution {
            sig12,
            s12: g.a * lam12,
            m12: g.b * sig12.sin(),
            big_m12: sig12.cos(),
            big_m21: sig12.cos(),
            alp1: (1.0_f64, 0.0_f64),
            alp2: (1.0_f64, 0.0_f64),
            omega: Some(sig12.sin_cos()),
        };
    }
    let first = start(g, k.ends, lam12, k.lam);
    if first.sig12 < 0.0_f64 {
        return iterate(g, k.ends, first.alp1, k.lam);
    }
    let dnm = first.dnm;
    let sig = first.sig12 / dnm;
    Solution {
        sig12: first.sig12,
        s12: first.sig12 * g.b * dnm,
        m12: dnm * dnm * g.b * sig.sin(),
        big_m12: sig.cos(),
        big_m21: sig.cos(),
        alp1: first.alp1,
        alp2: first.alp2,
        omega: Some((lam12 / (g.f1 * dnm)).sin_cos()),
    }
}

/// S12, Karney 2013 eqs. 58-60 and 62.
fn area(g: &Geodesic, k: &Canonical, s: &Solution) -> f64 {
    let (p1, p2) = k.ends;
    let ((salp1, calp1), (salp2, calp2)) = (s.alp1, s.alp2);
    let salp0 = salp1 * p1.c;
    let calp0 = calp1.hypot(salp1 * p1.s);
    let mut s12 = 0.0_f64;
    if calp0 != 0.0_f64 && salp0 != 0.0_f64 {
        let (ssig1, csig1) = norm(p1.s, calp1 * p1.c);
        let (ssig2, csig2) = norm(p2.s, calp2 * p2.c);
        let a4 = g.a * g.a * calp0 * salp0 * g.e2;
        let c4 = series::c4(&g.c4x, epsilon(g, calp0));
        s12 = a4 * (sin_cos_series(false, ssig2, csig2, &c4) - sin_cos_series(false, ssig1, csig1, &c4));
    }
    let fast = s.omega.filter(|&(_, comg12)| comg12 > AREA_COS && p2.s - p1.s < AREA_SPAN);
    let alp12 = if let Some((somg12, comg12)) = fast {
        let (domg12, dbet1, dbet2) = (1.0_f64 + comg12, 1.0_f64 + p1.c, 1.0_f64 + p2.c);
        TWO * (somg12 * (p1.s * dbet2 + p2.s * dbet1)).atan2(domg12 * (p1.s * p2.s + dbet1 * dbet2))
    } else {
        let mut sin_between = salp2 * calp1 - calp2 * salp1;
        let mut cos_between = calp2 * calp1 + salp2 * salp1;
        if sin_between == 0.0_f64 && cos_between < 0.0_f64 {
            sin_between = tiny() * calp1;
            cos_between = -1.0_f64;
        }
        sin_between.atan2(cos_between)
    };
    (s12 + g.c2 * alp12) * k.swapp * k.lonsign * k.latsign + 0.0_f64
}

/// The inverse problem between [lat, lon] `first` and `second`, degrees.
pub(super) fn inverse(g: &Geodesic, first: [f64; 2], second: [f64; 2]) -> Inverse {
    let k = canonical(g, first, second);
    let s = solve(g, &k);
    let area = area(g, &k, &s);
    let (mut start_alpha, mut end_alpha) = (s.alp1, s.alp2);
    let (mut scale_start, mut scale_end) = (s.big_m12, s.big_m21);
    if k.swapp < 0.0_f64 {
        std::mem::swap(&mut start_alpha, &mut end_alpha);
        std::mem::swap(&mut scale_start, &mut scale_end);
    }
    let fix = |(sin, cos): (f64, f64)| math::atan2d(sin * k.swapp * k.lonsign, cos * k.swapp * k.latsign);
    Inverse {
        distance: s.s12 + 0.0_f64,
        azimuth1: fix(start_alpha),
        azimuth2: fix(end_alpha),
        arc: s.sig12 / DEGREE,
        reduced: s.m12 + 0.0_f64,
        scale12: scale_start,
        scale21: scale_end,
        area,
    }
}
