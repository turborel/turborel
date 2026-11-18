//! Fixed-seed property tests of the DE-9IM and the four operations on
//! random small integer inputs, against the brute-force exact oracle.
//! Inputs on a small grid are dense in shared edges, touching vertices,
//! collinear overlaps and crossings at positions binary64 cannot hold.

use super::super::label::Location;
use super::super::{Geometry, Operation, Point, distance, overlay, relate};
use super::Seeded;
use super::oracle::{Rational, Shape, samples};

const SIDE: u64 = 4;
/// Within this of the result, a sample counts as in it; the oracle's
/// samples lie far farther from anything they are not on.
const NEAR: f64 = 1e-9;

fn coordinate(random: &mut Seeded, side: u64) -> f64 {
    f64::from(u32::try_from(random.below(side.checked_add(1).unwrap())).unwrap())
}

fn polyline(random: &mut Seeded) -> Vec<Point> {
    let count = random.below(2).checked_add(2).unwrap();
    let mut line: Vec<Point> = vec![random.point(SIDE)];
    while line.len() < usize::try_from(count).unwrap() {
        let next = random.point(SIDE);
        if line.last().is_some_and(|last| last.iter().zip(next).any(|(old, new)| (old - new).abs() > 0.5_f64)) {
            line.push(next);
        }
    }
    line
}

/// A rectangle, sometimes with a triangular hole strictly inside it, or
/// a triangle.
fn polygon(random: &mut Seeded) -> Vec<Vec<Point>> {
    if random.below(2) == 0 {
        loop {
            let [a, b, c] = [0_i32; 3].map(|_| random.point(SIDE));
            let turn = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if turn.abs() > 0.5_f64 {
                return vec![vec![a, b, c, a]];
            }
        }
    }
    let (x, y) = (coordinate(random, SIDE - 1), coordinate(random, SIDE - 1));
    let (w, h) = (1.0_f64 + coordinate(random, 2), 1.0_f64 + coordinate(random, 2));
    let shell = vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h], [x, y]];
    if w > 2.5_f64 && h > 2.5_f64 && random.below(2) == 0 {
        let hole = vec![[x + 1.0_f64, y + 1.0_f64], [x + 2.0_f64, y + 1.0_f64], [x + 1.0_f64, y + 2.0_f64], [x + 1.0_f64, y + 1.0_f64]];
        return vec![shell, hole];
    }
    vec![shell]
}

fn geometry(random: &mut Seeded) -> Geometry {
    match random.below(6) {
        0 => Geometry::Point(random.point(SIDE)),
        1 => Geometry::MultiPoint(vec![random.point(SIDE), random.point(SIDE)]),
        2 => Geometry::Line(polyline(random)),
        3 => Geometry::MultiLine(vec![polyline(random), polyline(random)]),
        _ => Geometry::Polygon(polygon(random)),
    }
}

fn index(location: Location) -> usize {
    location.index()
}

/// The oracle's matrix, per pair of locations the greatest dimension of
/// a stratum sampled there.
fn oracle_matrix(first: &Geometry, second: &Geometry) -> String {
    let (one, two) = (Shape::of(first), Shape::of(second));
    let mut cells = [[-1_i8; 3]; 3];
    cells[2][2] = 2;
    for sample in samples(&one, &two).0 {
        let cell = &mut cells[index(one.locate(sample.at))][index(two.locate(sample.at))];
        *cell = (*cell).max(sample.dimension);
    }
    cells
        .iter()
        .flatten()
        .map(|cell| if *cell < 0 { 'F' } else { char::from(b'0'.checked_add(u8::try_from(*cell).unwrap()).unwrap()) })
        .collect()
}

/// Verifies: [ISO/IEC 13249-3:2016 §4.2.2.1] the DE-9IM of random pairs
/// of every dimension equals the exact oracle's, cell for cell.
#[test]
fn relate_matches_the_oracle() {
    let mut random = Seeded(2016);
    for case in 0_i32..1_500_i32 {
        let (first, second) = (geometry(&mut random), geometry(&mut random));
        let engine = relate(&first, &second).unwrap().unwrap().code();
        assert_eq!(engine, oracle_matrix(&first, &second), "case {case}: {} / {}", super::text(&first), super::text(&second));
    }
}

fn member(shape: (&Shape, &Shape), operation: Operation, at: Rational) -> bool {
    let (one, two) = shape;
    operation.keeps([one.locate(at) != Location::Exterior, two.locate(at) != Location::Exterior])
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.31, §5.1.33, §5.1.35, §5.1.37]
/// each operation's result on random pairs holds exactly the oracle's
/// closure of the operation's point set at every stratum sample. A
/// sample is in the closure when it, or a position nudged from it, is
/// in the set. Nudges go along fixed directions, toward every input
/// vertex, so along every segment through the sample, and between each
/// two of those, so into every sector of the arrangement around it.
#[test]
fn operations_match_the_oracle() {
    let mut random = Seeded(13249);
    let fixed: [[i128; 2]; 8] = [[1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0], [-1, -1], [0, -1], [1, -1]];
    for case in 0_i32..400_i32 {
        let (first, second) = (geometry(&mut random), geometry(&mut random));
        let (one, two) = (Shape::of(&first), Shape::of(&second));
        let (strata, vertices) = samples(&one, &two);
        for operation in [Operation::Intersection, Operation::Union, Operation::Difference, Operation::SymDifference] {
            let result = overlay(&first, &second, operation).unwrap();
            for sample in &strata {
                let set = |at: Rational| member((&one, &two), operation, at);
                let towards: Vec<[i128; 2]> = vertices.iter().map(|vertex| sample.at.toward(*vertex)).collect();
                let between = towards.iter().enumerate().flat_map(|(index, one_way)| {
                    let sum = |left: i128, right: i128| left.checked_add(right).unwrap();
                    towards
                        .iter()
                        .skip(index.checked_add(1).unwrap())
                        .map(move |other| [sum(one_way[0], other[0]), sum(one_way[1], other[1])])
                });
                let expected = set(sample.at)
                    || fixed.iter().any(|direction| set(sample.at.nudged(*direction)))
                    || towards.iter().copied().chain(between).any(|direction| set(sample.at.along(direction)));
                let near = distance(&Geometry::Point(sample.at.approximate()), &result).is_some_and(|gap| gap <= NEAR);
                assert_eq!(
                    near,
                    expected,
                    "case {case} {operation:?} at {:?}: {} / {} gave {}",
                    sample.at.approximate(),
                    super::text(&first),
                    super::text(&second),
                    super::text(&result)
                );
            }
        }
    }
}
