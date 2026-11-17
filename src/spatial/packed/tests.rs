//! Hierarchical searches preserve exact closed-envelope candidates and
//! original row identities across partitions and mask word boundaries.

use super::*;

fn found(index: &Packed, bounds: Bounds) -> Vec<usize> {
    let mut cursor = Cursor::default();
    let mut rows = Vec::new();
    while let Some(row) = index.next(bounds, &mut cursor).unwrap() {
        rows.push(row);
    }
    assert!(index.next(bounds, &mut cursor).unwrap().is_none());
    rows.sort_unstable();
    rows
}

fn rectangles(count: usize) -> Vec<Option<Bounds>> {
    (0..count)
        .map(|at| {
            let x = f64::from(u32::try_from(at.saturating_mul(37) % 101).unwrap());
            let y = f64::from(u32::try_from(at.saturating_mul(13) % 97).unwrap());
            match at % 5 {
                0 => None,
                1 => Some(Bounds::point([x, y]).unwrap()),
                2 => Some(Bounds::new([-f64::MAX, y], [f64::MAX, y]).unwrap()),
                3 => Some(Bounds::new([x, y], [x + 20.0_f64, y + 20.0_f64]).unwrap()),
                _ => Some(Bounds::new([0.0_f64, 0.0_f64], [100.0_f64, 100.0_f64]).unwrap()),
            }
        })
        .collect()
}

/// Verifies: [DESIGN] D85: temporary spatial groups return exactly the
/// overlapping closed envelopes, once each, in their original row
/// positions. NULL bounds, coincident shapes, boundary touches, extreme
/// finite coordinates and inputs beyond a mask word retain every match.
#[test]
fn packed_masks_match_linear_search_across_group_boundaries() {
    let queries: Vec<_> = rectangles(200)
        .into_iter()
        .flatten()
        .chain([Bounds::point([-1.0_f64, -1.0_f64]).unwrap(), Bounds::point([f64::MAX, 1.0_f64]).unwrap()])
        .collect();
    for count in [0_usize, 1, 63, 64, 65, 127, 128, 129, 255, 256, 257, 1025] {
        let rectangles = rectangles(count);
        let index = Packed::new(&rectangles).unwrap();
        for &query in &queries {
            let expected: Vec<_> = rectangles
                .iter()
                .enumerate()
                .filter_map(|(at, bounds)| bounds.is_some_and(|bounds| bounds.intersects(query)).then_some(at))
                .collect();
            assert_eq!(found(&index, query), expected, "{count} bounds, {query:?}");
        }
    }
}

/// Verifies: [DESIGN] D38, D85: the hierarchy allocation estimate covers
/// retained vectors and leaf boundaries for every tested split size.
/// Empty bounds need no nodes; unrepresentable budgets saturate and can
/// be refused before allocation.
#[test]
fn packed_allocations_fit_the_reserved_bound() {
    for count in [0_usize, 1, 64, 65, 96, 128, 129, 193, 256, 257, 513, 1025] {
        let bounds = Bounds::point([0.0_f64, 0.0_f64]).unwrap();
        let index = Packed::new(&vec![Some(bounds); count]).unwrap();
        let retained = size_of::<Packed>()
            .saturating_add(index.items.capacity().saturating_mul(size_of::<Item>()))
            .saturating_add(index.nodes.capacity().saturating_mul(size_of::<Node>()))
            .saturating_add(count.saturating_mul(Masks::ENTRY_BYTES));
        assert!(retained <= Packed::bytes(count));
        assert_eq!(found(&index, bounds), (0..count).collect::<Vec<_>>());
        assert!(Packed::new(&vec![None; count]).unwrap().nodes.is_empty());
    }
    assert_eq!(Packed::bytes(usize::MAX), usize::MAX);
}
