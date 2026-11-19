//! Deterministic realistic geometry: parcels, star-shaped polygons of 32
//! to 256 vertices whose radius varies with angle, on a jittered grid; and
//! a road network, wiggling polylines of 17 vertices along the grid's
//! lines, both ways. No data is downloaded; the same seed gives the same
//! shapes every run.

use super::harness::scatter;

/// The coordinate domain's side.
pub const SIDE: f64 = 10_000.0_f64;
/// Parcels along each side of the grid, and roads each way.
const PARCEL_SIDE: u64 = 150;
const ROAD_LINES: u64 = 60;
/// Pieces of a road between two crossings, and vertices of each piece.
const PIECES: u64 = 30;
const PIECE_VERTICES: u64 = 17;

/// The parcels, `PARCEL_SIDE` squared, as WKT.
pub fn parcels() -> Vec<String> {
    (0..PARCEL_SIDE * PARCEL_SIDE).map(parcel).collect()
}

/// The parcel `n`: a star-shaped ring about a jittered cell centre.
pub fn parcel(n: u64) -> String {
    let cell = SIDE / f(PARCEL_SIDE);
    let jitter = |k: u64| (f(scatter(n * 31 + k, 1000)) / 1000.0_f64 - 0.5_f64) * cell * 0.3_f64;
    let (cx, cy) = ((f(n % PARCEL_SIDE) + 0.5_f64) * cell + jitter(1), (f(n / PARCEL_SIDE) + 0.5_f64) * cell + jitter(2));
    ring(n, (cx, cy), cell * 0.35_f64)
}

/// The parcel `n` moved by `step` cells' widths to the right, wrapping.
pub fn moved(n: u64, step: u64) -> String {
    let cell = SIDE / f(PARCEL_SIDE);
    let x = f((n % PARCEL_SIDE + step) % PARCEL_SIDE);
    ring(n, ((x + 0.5_f64) * cell, (f(n / PARCEL_SIDE) + 0.5_f64) * cell), cell * 0.35_f64)
}

fn ring(n: u64, (cx, cy): (f64, f64), radius: f64) -> String {
    let vertices = 32_u64 << (n % 4);
    let lobes = 3 + n % 5;
    let points: Vec<String> = (0..=vertices)
        .map(|k| {
            let angle = std::f64::consts::TAU * f(k % vertices) / f(vertices);
            let r = radius * (1.0_f64 + 0.3_f64 * (f(lobes) * angle + f(n)).sin());
            format!("{} {}", cx + r * angle.cos(), cy + r * angle.sin())
        })
        .collect();
    format!("POLYGON (({}))", points.join(", "))
}

/// The road pieces, each way along `ROAD_LINES` lines, as WKT.
pub fn roads() -> Vec<String> {
    let mut out = Vec::new();
    for line in 0..ROAD_LINES {
        for piece in 0..PIECES {
            out.push(road(line, piece, false));
            out.push(road(line, piece, true));
        }
    }
    out
}

fn road(line: u64, piece: u64, vertical: bool) -> String {
    let at = (f(line) + 0.37_f64) * SIDE / f(ROAD_LINES);
    let length = SIDE / f(PIECES);
    let points: Vec<String> = (0..PIECE_VERTICES)
        .map(|k| {
            let along = f(piece) * length + f(k) * length / f(PIECE_VERTICES - 1);
            let wiggle = 12.0_f64 * ((along / 90.0_f64) + f(line)).sin();
            if vertical { format!("{} {along}", at + wiggle) } else { format!("{along} {}", at + wiggle) }
        })
        .collect();
    format!("LINESTRING ({})", points.join(", "))
}

/// The square window of side `width` at a scattered place.
pub fn window(n: u64, width: f64) -> String {
    let x = f(scatter(n * 7 + 3, 10_000)) / 10_000.0_f64 * (SIDE - width);
    let y = f(scatter(n * 13 + 5, 10_000)) / 10_000.0_f64 * (SIDE - width);
    format!("POLYGON (({x} {y}, {} {y}, {} {}, {x} {}, {x} {y}))", x + width, x + width, y + width, y + width)
}

/// `n` as a coordinate; every count here is far below 2^32.
fn f(n: u64) -> f64 {
    f64::from(u32::try_from(n).unwrap())
}
