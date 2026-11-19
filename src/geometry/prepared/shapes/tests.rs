//! Filed-edge intersection checked against the planar engine's for lines,
//! polygons and their multi-part forms against prepared polygons, holes
//! and containment both ways included.

use super::*;
use crate::geometry::crossing;
use crate::geometry::shape::MULTILINE;
use crate::geometry::tests::common::dense_ring;

fn square(low: f64, high: f64) -> Shape {
    Shape::Polygon(vec![dense_ring(low, high)])
}

fn holed() -> Shape {
    let mut hole = dense_ring(2.0_f64, 8.0_f64);
    hole.reverse();
    Shape::Polygon(vec![dense_ring(0.0_f64, 10.0_f64), hole])
}

/// Operands about the prepared polygons: inside, in the hole, crossing,
/// containing, disjoint, touching, lines across and within, and
/// multi-part and empty forms.
fn operands() -> Vec<Shape> {
    let line = |points: &[Point]| Shape::Line(points.to_vec());
    let mut shapes = vec![
        square(3.0_f64, 7.0_f64),
        square(0.5_f64, 1.5_f64),
        square(-5.0_f64, 2.5_f64),
        square(-20.0_f64, 40.0_f64),
        square(30.0_f64, 31.0_f64),
        square(10.0_f64, 12.0_f64),
        Shape::Polygon(Vec::new()),
        line(&[[-1.0_f64, 5.0_f64], [11.0_f64, 5.0_f64]]),
        line(&[[4.0_f64, 4.0_f64], [6.0_f64, 6.0_f64]]),
        line(&[[0.5_f64, 0.5_f64], [1.0_f64, 1.5_f64]]),
        line(&[]),
    ];
    let parts = vec![square(3.0_f64, 7.0_f64), square(30.0_f64, 31.0_f64)];
    shapes.push(Shape::Multi(MULTIPOLYGON, parts));
    let lines = vec![line(&[[30.0_f64, 30.0_f64], [31.0_f64, 31.0_f64]]), line(&[[9.0_f64, -1.0_f64], [9.0_f64, 11.0_f64]])];
    shapes.push(Shape::Multi(MULTILINE, lines));
    shapes
}

/// The planar engine's intersection of two shapes, none of an empty one.
fn engine_intersects(left: &Shape, right: &Shape) -> Option<bool> {
    use crate::geometry::engine::of_shape;
    crate::spatial::planar::intersects(&of_shape(left), &of_shape(right)).unwrap()
}

/// Verifies: [DESIGN] D85: a prepared polygon's filed edges decide
/// intersection exactly as the planar engine's relationship does, for a holed
/// polygon and a multipolygon against polygons inside, in the hole,
/// crossing, containing, touching and apart, empty polygons, lines and
/// their multi-part forms; collections and points are left to the
/// general path.
#[test]
fn filed_edges_decide_as_the_relationship_does() {
    let budget = MemoryBudget::new(usize::MAX);
    let multi = Shape::Multi(MULTIPOLYGON, vec![holed(), square(20.0_f64, 25.0_f64)]);
    for prepared in [holed(), multi] {
        let envelope = prepared.bounds().unwrap().unwrap();
        let filed = FiledEdges::new(&prepared, envelope, &budget).expect("filed edges");
        let locate = |point| crossing::locate(point, &prepared);
        for other in operands() {
            let decided = filed.meets(&prepared, &other, &locate);
            // The engine's intersection is the null value of an empty operand.
            match engine_intersects(&prepared, &other) {
                Some(expected) => assert_eq!(decided, Some(expected), "{other:?}"),
                None => assert_ne!(decided, Some(true), "{other:?}"),
            }
        }
        let collection = Shape::Multi(COLLECTION, vec![square(3.0_f64, 7.0_f64)]);
        assert_eq!(filed.meets(&prepared, &collection, &locate), None);
        assert_eq!(filed.meets(&prepared, &Shape::Point(Some([5.0_f64, 5.0_f64])), &locate), None);
    }
    assert_eq!(budget.reserved(), 0, "released with the filed edges");
}
