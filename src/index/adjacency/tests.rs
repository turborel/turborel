use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;

use super::*;
use crate::engine::MemoryPages;
use crate::engine::tree::Direction as Order;
use crate::memory::MemoryBudget;
use crate::value::{Decimal, Spelled, Value};

mod compacted;
mod reads;

/// An INTEGER column.
pub(super) fn int() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }
}

pub(super) fn n(v: i128) -> Value {
    Value::Exact(Decimal::new(v, 0))
}

/// The key of an edge from `src` to `dst` of property `amount`.
pub(super) fn edge(src: i128, dst: i128, amount: i128) -> Key {
    Key(vec![n(src), n(dst), n(amount)])
}

/// A method of its own pages in memory, its adjacencies held of a budget
/// of `bytes`, if given.
pub(super) fn method(bytes: Option<usize>) -> AdjacencyMethod {
    let made = AdjacencyMethod::sharing(MemoryPages::new(MEMORY_POOL_BYTES));
    match bytes {
        Some(b) => made.with_budget(MemoryBudget::new(b)),
        None => made,
    }
}

/// An index of `method`, its key (source, destination, an included
/// amount), kept as `kept` says.
pub(super) fn index(method: &AdjacencyMethod, kept: Kept) -> Arc<AdjacencyIndex> {
    let layout = Layout::new(&[int(), int(), int()], (1, 1), kept).unwrap();
    let (pool, file) = method.files().unwrap();
    Arc::new(method.index_of(BLinkTree::create(pool, file).unwrap(), layout, (KeyCounts::new(), DEFAULT_CSR_BYTES)))
}

/// The next of a seeded sequence of pseudo-random numbers.
pub(super) fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

/// A seeded random edge among `vertices` vertices, of amount 0 to 2 and
/// a row below `rows`.
pub(super) fn random_edge(state: &mut u64, vertices: u64, rows: u64) -> (i128, i128, i128, RowId) {
    let mut pick = |m: u64| i128::from(next(state).checked_rem(m).unwrap());
    (pick(vertices), pick(vertices), pick(3), u64::try_from(pick(rows)).unwrap())
}

/// What the index should hold: each (source, destination, amount, row)
/// entry with its versions.
#[derive(Default, Clone)]
pub(super) struct Model(pub(super) BTreeMap<(i128, i128, i128, RowId), u32>);

impl Model {
    pub(super) fn insert(&mut self, index: &AdjacencyIndex, (s, d, a, row): (i128, i128, i128, RowId)) {
        index.insert(edge(s, d, a), row).unwrap();
        let v = self.0.entry((s, d, a, row)).or_default();
        *v = v.saturating_add(1);
    }

    pub(super) fn remove(&mut self, index: &AdjacencyIndex, (s, d, a, row): (i128, i128, i128, RowId)) {
        index.remove(&edge(s, d, a), row).unwrap();
        if let Some(v) = self.0.get_mut(&(s, d, a, row)) {
            *v = v.saturating_sub(1);
            if *v == 0 {
                self.0.remove(&(s, d, a, row));
            }
        }
    }

    /// The steps of `vertex` in `direction`: (other end, row), each once,
    /// in the order of the other end, then the row.
    pub(super) fn steps(&self, vertex: i128, direction: Direction) -> Vec<(i128, RowId)> {
        let mut steps: Vec<(i128, RowId)> = self
            .0
            .keys()
            .filter_map(|&(s, d, _, row)| match direction {
                Direction::Out => (s == vertex).then_some((d, row)),
                Direction::In => (d == vertex).then_some((s, row)),
            })
            .collect();
        steps.sort_unstable();
        steps.dedup();
        steps
    }

    /// The entries under `vertex` in `direction`.
    pub(super) fn degree(&self, vertex: i128, direction: Direction) -> u64 {
        let under = |&&(s, d, _, _): &&(i128, i128, i128, RowId)| match direction {
            Direction::Out => s == vertex,
            Direction::In => d == vertex,
        };
        u64::try_from(self.0.keys().filter(under).count()).unwrap()
    }
}

/// The steps `index` gives of `vertex` in `direction`, as the model's.
pub(super) fn steps(index: &AdjacencyIndex, vertex: i128, direction: Direction) -> Vec<(i128, RowId)> {
    let mut out = Vec::new();
    index.steps(&Key(vec![n(vertex)]), direction, &mut out).unwrap();
    out.into_iter()
        .map(|s| match s.vertex.0.as_slice() {
            [Value::Exact(d)] => (d.to_string().parse().unwrap(), s.edge),
            other => panic!("{other:?}"),
        })
        .collect()
}

/// Checks every vertex's steps and degree, both ways, against the model;
/// and its entries in the tree, every key with its value.
pub(super) fn agrees(index: &AdjacencyIndex, model: &Model, vertices: i128, what: &str) {
    for v in 0..vertices {
        for direction in [Direction::Out, Direction::In] {
            assert_eq!(steps(index, v, direction), model.steps(v, direction), "{what}: {v} {direction:?}");
            assert_eq!(index.degree(&Key(vec![n(v)]), direction).unwrap(), model.degree(v, direction), "{what}: {v} {direction:?}");
        }
    }
}

/// Every key and value of the tree of `index`.
pub(super) fn dump(index: &AdjacencyIndex) -> Vec<(Vec<u8>, Vec<u8>)> {
    index.tree.scan(Bound::Unbounded, Bound::Unbounded, Order::Forward).map(Result::unwrap).collect()
}

/// Verifies: [DESIGN] RULES.md D29: entries are counted by version, a
/// pair inserted twice staying until removed twice, its vertex's degree
/// counting it once; versions of an edge differing only in an included
/// property are one step, and two entries of its degree.
#[test]
fn versions_are_counted_and_steps_made_once() {
    let method = method(None);
    let ix = index(&method, Kept::Both);
    let mut model = Model::default();
    model.insert(&ix, (1, 2, 10, 7));
    model.insert(&ix, (1, 2, 10, 7));
    model.insert(&ix, (1, 2, 11, 7));
    model.insert(&ix, (1, 3, 10, 8));
    agrees(&ix, &model, 4, "inserted");
    assert_eq!(steps(&ix, 1, Direction::Out), [(2, 7), (3, 8)]);
    assert_eq!(ix.degree(&Key(vec![n(1)]), Direction::Out).unwrap(), 3);
    model.remove(&ix, (1, 2, 10, 7));
    agrees(&ix, &model, 4, "one version taken");
    assert_eq!(ix.rows(&edge(1, 2, 10)).unwrap(), [7], "one version left");
    model.remove(&ix, (1, 2, 10, 7));
    model.remove(&ix, (1, 2, 11, 7));
    model.remove(&ix, (1, 2, 11, 7));
    agrees(&ix, &model, 4, "all versions taken");
    assert_eq!(steps(&ix, 2, Direction::In), []);
    assert_eq!(ix.rows(&edge(1, 3, 10)).unwrap(), [8]);
    assert_eq!(ix.edges(), 1);
    assert_eq!(dump(&ix).len(), 4, "an entry and a degree each way");
}

/// Verifies: [DESIGN] of seeded random inserts and removes, of versions
/// among them, each vertex's steps and degree, both ways, are those of
/// every edge held, as a scan of them gives; an index kept one way
/// follows the other way too, by reading its entries whole.
#[test]
fn steps_are_the_edges_held() {
    for kept in [Kept::Both, Kept::Out, Kept::In] {
        let method = method(None);
        let ix = index(&method, kept);
        let (mut model, mut state) = (Model::default(), 5_u64);
        for _ in 0_u32..2_000 {
            let e = random_edge(&mut state, 30, 50);
            if next(&mut state).is_multiple_of(4) {
                model.remove(&ix, e);
            } else {
                model.insert(&ix, e);
            }
        }
        agrees(&ix, &model, 31, &format!("{kept:?}"));
    }
}

/// Verifies: [DESIGN] RULES.md D29, D34: a load leaves the index as
/// inserting each entry would, an entry given twice counted as two
/// versions, degrees and every entry alike.
#[test]
fn a_load_is_as_inserts() {
    let method = method(None);
    let (loaded, inserted) = (index(&method, Kept::Both), index(&method, Kept::Both));
    let (mut model, mut state) = (Model::default(), 9_u64);
    let mut all = Vec::new();
    for _ in 0_u32..3_000 {
        let e = random_edge(&mut state, 100, 400);
        // One in ten given twice, two versions of the row.
        for _ in 0..=usize::from(next(&mut state).is_multiple_of(10)) {
            model.insert(&inserted, e);
            all.push(Ok((edge(e.0, e.1, e.2), e.3)));
        }
    }
    let work = WorkAreas::new(MemoryBudget::new(usize::MAX));
    loaded.load(&mut all.into_iter().rev(), (&work, Area::new(4_096, 64))).unwrap();
    agrees(&loaded, &model, 101, "loaded");
    assert_eq!(dump(&loaded), dump(&inserted));
}

/// Verifies: [EXTENSION] RULES.md D41: a locator reads back its layout
/// and key counts, and an index opened of it holds its entries; another
/// version is refused; a layout with no source or no destination column
/// is refused.
#[test]
fn a_locator_reads_back_and_layouts_are_checked() {
    let method = method(None);
    let ix = index(&method, Kept::In);
    ix.insert(edge(1, 2, 3), 4).unwrap();
    let opened = method.open(&[int(), int(), int()], &ix.locator()).unwrap().unwrap();
    assert_eq!(opened.rows(&edge(1, 2, 3)).unwrap(), [4]);
    assert_eq!(opened.locator(), ix.locator());
    let mut other = ix.locator();
    other[0] = 9;
    assert!(method.open(&[int(), int(), int()], &other).is_err());
    assert!(method.create(&[int()]).is_err(), "no destination");
    assert!(Layout::new(&[int(), int()], (1, 1), Kept::Both).is_err(), "the destination included");
    assert!(Layout::new(&[int(), int()], (0, 0), Kept::Both).is_err(), "no source");
    assert!(Layout::new(&[int(), int(), int()], (2, 0), Kept::Both).is_ok());
}
