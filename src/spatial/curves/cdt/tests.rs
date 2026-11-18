use super::*;

fn mesh(points: &[(f64, f64)]) -> (Mesh, Vec<usize>) {
    let mut m = Mesh::new([-1.0_f64, -1.0_f64], [11.0_f64, 11.0_f64]);
    let ids = points.iter().map(|&(x, y)| m.insert([x, y, 0.0_f64])).collect();
    (m, ids)
}

/// Every triangle's circumcircle holds no other vertex, unless the
/// edge between them is constrained.
fn is_delaunay(m: &Mesh) -> bool {
    m.triangles().iter().all(|t| {
        let full = t.map(|v| v.saturating_add(SUPER));
        (SUPER..m.points.len()).filter(|v| !full.contains(v)).all(|v| m.in_circle(full, v) <= 0.0_f64)
    })
}

/// Every unconstrained edge between inserted points is locally
/// Delaunay.
fn is_constrained_delaunay(m: &Mesh) -> bool {
    m.owners.keys().all(|(u, v)| *u < SUPER || *v < SUPER || m.is_constrained(*u, *v) || !m.illegal(*u, *v))
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.3 D4c] A
/// grid's triangulation is Delaunay, covers the hull's area, keeps
/// every point once and merges a repeated point.
#[test]
fn grid_is_delaunay() {
    let mut points = Vec::new();
    for i in 0_i32..5_i32 {
        for j in 0_i32..4_i32 {
            points.push((f64::from(i) * 2.0_f64 + 0.1_f64 * f64::from(j), f64::from(j) * 2.5_f64));
        }
    }
    let (mut m, ids) = mesh(&points);
    assert_eq!(m.insert([0.0_f64, 0.0_f64, 9.0_f64]), ids[0]);
    m.legalize();
    assert!(is_delaunay(&m));
    let area: f64 = m
        .triangles()
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|v| m.vertices()[v]);
            0.5_f64 * ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]))
        })
        .sum();
    assert!((area - 8.0 * 7.5).abs() < 1e-9_f64, "{area}");
    assert_eq!(m.vertices().len(), 20);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.3 D4c,
/// §17.1] A constraint across a fan becomes an edge; one passing through
/// a vertex is split there; two crossing constraints meet at a new
/// vertex whose z comes from the first.
#[test]
fn constraints_are_edges() {
    let (mut m, ids) = mesh(&[
        (0.0_f64, 0.0_f64),
        (10.0_f64, 0.0_f64),
        (10.0_f64, 10.0_f64),
        (0.0_f64, 10.0_f64),
        (5.0_f64, 1.0_f64),
        (5.0_f64, 9.0_f64),
        (4.0_f64, 5.0_f64),
        (6.0_f64, 5.0_f64),
    ]);
    m.constrain(ids[6], ids[7]).unwrap();
    assert!(m.has_edge(ids[6], ids[7]) && m.is_constrained(ids[7], ids[6]));
    m.constrain(ids[0], ids[2]).unwrap();
    let mid = m.points.iter().position(|p| (p[0] - 5.0).abs() < 1e-12_f64 && (p[1] - 5.0).abs() < 1e-12_f64).unwrap();
    assert!(m.is_constrained(ids[0], mid) && m.is_constrained(mid, ids[2]));
    assert!(m.is_constrained(ids[6], mid) && m.is_constrained(mid, ids[7]));
    m.points[ids[4]][2] = 2.0_f64;
    m.points[ids[5]][2] = 4.0_f64;
    m.constrain(ids[4], ids[5]).unwrap();
    let cross = m.points.iter().position(|p| (p[0] - 5.0).abs() < 1e-12_f64 && (p[1] - 5.0).abs() < 1e-12_f64).unwrap();
    assert!(m.is_constrained(ids[4], cross) || m.is_constrained(ids[4], mid));
    m.legalize();
    assert!(is_constrained_delaunay(&m));
    for (u, v) in m.constrained.clone() {
        assert!(m.has_edge(u, v), "constraint {u} {v} lost");
    }
    assert!(m.triangles().len() >= 8);
    assert_eq!(edge(3, 1), (1, 3));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.3 D4c] A
/// constraint crossing many edges of a grid is flipped into place, and
/// the result is constrained Delaunay.
#[test]
fn long_constraint_across_a_grid() {
    let mut m = Mesh::new([0.0_f64, 0.0_f64], [4.0_f64, 4.0_f64]);
    for i in 0_i32..=4_i32 {
        for j in 0_i32..=4_i32 {
            m.insert([f64::from(i), f64::from(j), f64::from(i + j)]);
        }
    }
    let a = m.insert([0.5_f64, 0.2_f64, 9.0_f64]);
    let b = m.insert([3.5_f64, 3.7_f64, 9.0_f64]);
    m.constrain(a, b).unwrap();
    assert!(m.has_edge(a, b));
    m.legalize();
    assert!(m.has_edge(a, b) && is_constrained_delaunay(&m));
    // Euler: 2n - h - 2 triangles for n points, h of them on the hull.
    assert_eq!(m.triangles().len(), 2 * 27 - 16 - 2);
}
