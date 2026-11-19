//! \[DESIGN\] An adjacency index read (RULES.md D34): a vertex's steps and
//! degree, for path search (`Adjacency`), and the entries of an equality
//! of the key's first columns, for the planner. A vertex's list is read of
//! the compacted adjacency unless the delta marks it, else of the tree
//! (`delta.rs`). An equality of the source's columns alone is a vertex's
//! forward list; of fewer, or of the destination's too, a prefix of the
//! forward entries; of an included column, that prefix, each key checked.
//! An index keeping reverse entries alone is read whole, each key checked.
//!
//! What a search gives holds the index, or the compacted adjacency it
//! reads, so neither is freed while it is read.

use std::collections::HashSet;
use std::ops::{Deref, Range};
use std::sync::Arc;

use super::AdjacencyIndex;
use super::csr::Csr;
use super::keys::{FORWARD, REVERSE, anchored, decode_all, degree_key, degree_of, parse, tag, within};
use super::options::Layout;
use crate::engine::tree::{BLinkTree, Direction as Order, Scan, scan_of};
use crate::error::{Code, Result, SqlError};
use crate::graph::adjacency::{Direction, Step};
use crate::index::{ColumnComparison, Entries, Key, Probe};
use crate::storage::RowId;
use crate::value::{CompOp, Value, compare};

/// An index's tree, reached through the index it holds: what a search
/// reads, so the index outlives the search.
pub struct Held(pub Arc<AdjacencyIndex>);

impl Deref for Held {
    type Target = BLinkTree;
    fn deref(&self) -> &BLinkTree {
        &self.0.tree
    }
}

/// The scan of the keys of `tree` beginning with `prefix`.
fn scan<T: Deref<Target = BLinkTree>>(tree: T, prefix: &[u8]) -> Scan<T> {
    let (low, high) = within(prefix);
    scan_of(tree, low.as_ref().map(Vec::as_slice), high.as_ref().map(Vec::as_slice), Order::Forward)
}

/// `prefix` after `tag`.
fn tagged(tag: u8, prefix: &[u8]) -> Vec<u8> {
    [&[tag], prefix].concat()
}

/// Whether `values` equal `key`'s by the comparison rules, each to each.
fn equal(values: &[Value], key: &[Value]) -> bool {
    values.len() == key.len() && values.iter().zip(key).all(|(a, b)| compare(a, b).is_some_and(std::cmp::Ordering::is_eq))
}

/// The error of a probe it does not answer.
fn refused() -> SqlError {
    SqlError::new(Code::Unsupported, "[EXTENSION] the adjacency index method answers equalities of its key's first columns and every entry")
}

/// The values a probe compares the key's first columns equal with; none
/// for every entry.
///
/// # Errors
///
/// SQLCODE -900 of a probe it does not answer.
fn equalities(probe: &Probe) -> Result<Option<Vec<Value>>> {
    let compared = |op: CompOp, values: Vec<Value>| if op == CompOp::Eq { Ok(Some(values)) } else { Err(refused()) };
    match probe {
        Probe::All => Ok(None),
        Probe::Compare { op, value } => compared(*op, vec![value.clone()]),
        Probe::Prefix { equal, then: None } => Ok(Some(equal.clone())),
        Probe::Prefix { equal, then: Some((op, value)) } => compared(*op, [equal.as_slice(), std::slice::from_ref(value)].concat()),
        Probe::Columns(_) | Probe::Range { .. } | Probe::Predicate { .. } | Probe::Xml(_) => Err(refused()),
    }
}

/// How the entries of an equality are read.
enum Plan {
    /// None: an equality with null.
    Nothing,
    /// A vertex's forward list: of its source's key, exact.
    Vertex(Vec<u8>),
    /// The entries beginning with `prefix`, each checked by the probe if
    /// it is given.
    Scan(Vec<u8>, Option<Probe>),
}

/// How the entries whose first columns equal `equal` are read in `layout`.
fn plan(layout: &Layout, equal: &[Value]) -> Plan {
    let check = Some(Probe::Prefix { equal: equal.to_vec(), then: None });
    if !layout.kept.keeps(Direction::Out) {
        return Plan::Scan(vec![tag(Direction::In)], check);
    }
    let fixed = equal.len().min(layout.endpoints());
    let Some((prefix, checked)) = anchored(equal.get(..fixed).unwrap_or_default(), &layout.types) else { return Plan::Nothing };
    if checked || equal.len() > fixed {
        return Plan::Scan(tagged(FORWARD, &prefix), check);
    }
    if equal.len() == layout.source { Plan::Vertex(prefix) } else { Plan::Scan(tagged(FORWARD, &prefix), None) }
}

/// The entries of `tree` beginning with `prefix`, keys in column order,
/// those `check` does not answer left out.
fn entries<T: Deref<Target = BLinkTree> + Send>(
    tree: T,
    layout: &Arc<Layout>,
    (prefix, check): (&[u8], Option<Probe>),
) -> impl Iterator<Item = Result<(Key, RowId)>> + Send + use<T> {
    let layout = Arc::clone(layout);
    scan(tree, prefix).filter_map(move |entry| {
        let found = entry.and_then(|(key, _)| {
            let parsed = parse(&key, &layout)?;
            Ok((parsed.key(&layout)?, parsed.row))
        });
        match found {
            Ok((key, row)) => check.as_ref().is_none_or(|p| p.answers(&key.0)).then_some(Ok((key, row))),
            Err(e) => Some(Err(e)),
        }
    })
}

/// A vertex's forward list read of a compacted adjacency, as its entries.
struct Compacted {
    csr: Arc<Csr>,
    layout: Arc<Layout>,
    /// The source's values, as its key holds them.
    source: Vec<Value>,
    at: Range<usize>,
}

impl Iterator for Compacted {
    type Item = Result<(Key, RowId)>;

    fn next(&mut self) -> Option<Self::Item> {
        let at = self.at.next()?;
        Some(self.csr.step(Direction::Out, at).and_then(|(other, row, included)| {
            let mut values = self.source.clone();
            values.extend(decode_all(other, self.layout.destination_types())?);
            values.extend(decode_all(included, self.layout.included_types())?);
            Ok((Key(values), row))
        }))
    }
}

/// `search`: every entry, those of an equality of the first columns, or,
/// of comparisons of other columns, those they answer: of equalities of
/// the destination's columns alone, a vertex's reverse list if it is kept
/// (\[GS §16 R16.1\] the search walked the other way), else every entry,
/// each checked.
pub fn search(index: &Arc<AdjacencyIndex>, probe: &Probe) -> Entries {
    if let Probe::Columns(each) = probe {
        let (prefix, check) = reverse_plan(&index.layout, each, probe);
        return Box::new(entries(Held(Arc::clone(index)), &index.layout, (&prefix, check)));
    }
    let equal = match equalities(probe) {
        Ok(Some(equal)) => equal,
        Ok(None) => {
            let first = index.layout.kept.directions().first().copied().unwrap_or(Direction::Out);
            return Box::new(entries(Held(Arc::clone(index)), &index.layout, (&[tag(first)], None)));
        }
        Err(e) => return Box::new(std::iter::once(Err(e))),
    };
    match plan(&index.layout, &equal) {
        Plan::Nothing => Box::new(std::iter::empty()),
        Plan::Vertex(vertex) => match (index.delta.csr(Direction::Out, &vertex), decode_all(&vertex, index.layout.source_types())) {
            (Ok(Some(csr)), Ok(source)) => {
                let at = csr.span(Direction::Out, &vertex);
                Box::new(Compacted { csr, layout: Arc::clone(&index.layout), source, at })
            }
            (Ok(None), Ok(_)) => Box::new(entries(Held(Arc::clone(index)), &index.layout, (&tagged(FORWARD, &vertex), None))),
            (Err(e), _) | (_, Err(e)) => Box::new(std::iter::once(Err(e))),
        },
        Plan::Scan(prefix, check) => Box::new(entries(Held(Arc::clone(index)), &index.layout, (&prefix, check))),
    }
}

/// How the entries `each` answers are read in `layout`: of equalities of
/// the destination's columns, in order, kept reverse, the destination's
/// reverse entries, unchecked if its bytes alone select them; else every
/// entry of the first direction kept, each checked by `probe`.
fn reverse_plan(layout: &Layout, each: &[ColumnComparison], probe: &Probe) -> (Vec<u8>, Option<Probe>) {
    let destination = layout.source..layout.endpoints();
    let exact = each.len() == destination.len()
        && each.iter().zip(destination).all(|(c, at)| c.column == at && c.op == CompOp::Eq)
        && layout.kept.keeps(Direction::In);
    let values: Vec<Value> = each.iter().map(|c| c.value.clone()).collect();
    match exact.then(|| anchored(&values, layout.destination_types())).flatten() {
        Some((prefix, false)) => (tagged(REVERSE, &prefix), None),
        Some((prefix, true)) => (tagged(REVERSE, &prefix), Some(probe.clone())),
        None => {
            let first = layout.kept.directions().first().copied().unwrap_or(Direction::Out);
            (vec![tag(first)], Some(probe.clone()))
        }
    }
}

/// The rows with an entry of `tree` under `key`, of `layout`: its vertex's
/// entries reaching its other end, those of its included columns.
///
/// # Errors
///
/// SQLCODE -910 if `key` is not of the layout.
pub fn rows_of<T: Deref<Target = BLinkTree> + Send>(
    tree: T,
    layout: &Arc<Layout>,
    key: &Key,
) -> Result<impl Iterator<Item = Result<RowId>> + Send + use<T>> {
    let parts = super::keys::Encoded::of(key, layout)?;
    let direction = layout.kept.directions().first().copied().unwrap_or(Direction::Out);
    let (vertex, other) = parts.ends(direction);
    let prefix = [&[tag(direction)], vertex, other].concat();
    let (layout, included) = (Arc::clone(layout), parts.included);
    Ok(scan(tree, &prefix).filter_map(move |entry| {
        let found = entry.and_then(|(bytes, _)| {
            let parsed = parse(&bytes, &layout)?;
            Ok((parsed.included == included.as_slice()).then_some(parsed.row))
        });
        found.transpose()
    }))
}

/// \[DESIGN\] The most entries an estimate counts, as the btree's: past
/// them, it is not cheap.
const MOST_COUNTED: usize = 1_000;

impl AdjacencyIndex {
    /// `estimate`: a vertex's degree for an equality of its source, else
    /// the entries counted, up to `MOST_COUNTED`.
    pub(super) fn estimated(&self, probe: &Probe) -> Option<usize> {
        let equal = equalities(probe).ok()??;
        let (prefix, check) = match plan(&self.layout, &equal) {
            Plan::Nothing => return Some(0),
            Plan::Vertex(vertex) => return self.listed(Direction::Out, &vertex).ok().and_then(|n| usize::try_from(n).ok()),
            Plan::Scan(prefix, check) => (prefix, check),
        };
        let mut counted = 0_usize;
        for entry in scan(&self.tree, &prefix) {
            let (key, _) = entry.ok()?;
            let answers = match check.as_ref() {
                Some(p) => p.answers(&parse(&key, &self.layout).and_then(|e| e.key(&self.layout)).ok()?.0),
                None => true,
            };
            counted = counted.saturating_add(usize::from(answers));
            if counted > MOST_COUNTED {
                return None;
            }
        }
        Some(counted)
    }

    /// The entries under the vertex of key bytes `vertex` in `direction`,
    /// of its compacted adjacency or its degree.
    fn listed(&self, direction: Direction, vertex: &[u8]) -> Result<u64> {
        if let Some(csr) = self.delta.csr(direction, vertex)? {
            return Ok(u64::try_from(csr.span(direction, vertex).len()).unwrap_or(u64::MAX));
        }
        self.tree.get(&degree_key(direction, vertex))?.map_or(Ok(0), |v| degree_of(&v))
    }

    /// `Adjacency::steps`: of a vertex's list, kept, its entries, one step
    /// of those alike but for their included columns, which are adjacent.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `vertex` is not a key of the vertex's columns; the
    /// tree's.
    pub(super) fn steps_of(&self, vertex: &Key, direction: Direction, out: &mut Vec<Step>) -> Result<()> {
        let other_types = self.layout.ends(direction).1;
        if self.layout.kept.keeps(direction)
            && let Some((prefix, false)) = self.anchored(vertex, direction)?
            && let Some(csr) = self.delta.csr(direction, &prefix)?
        {
            return csr.steps(direction, &prefix, other_types, out);
        }
        let mut seen: HashSet<(Vec<u8>, RowId)> = HashSet::new();
        let mut last: Option<(Vec<u8>, RowId)> = None;
        self.reaching(vertex, direction, &mut |other, row| {
            let step = (other.to_vec(), row);
            let again = if self.layout.kept.keeps(direction) { last.as_ref() == Some(&step) } else { !seen.insert(step.clone()) };
            if !again {
                out.push(Step { edge: row, vertex: Key(decode_all(other, other_types)?) });
                last = Some(step);
            }
            Ok(())
        })
    }

    /// The bytes of `vertex` as a prefix in `direction` (`anchored`).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `vertex` is not a key of the vertex's columns.
    pub(super) fn anchored(&self, vertex: &Key, direction: Direction) -> Result<Option<(Vec<u8>, bool)>> {
        let vertex_types = self.layout.ends(direction).0;
        if vertex.0.len() != vertex_types.len() {
            return Err(SqlError::new(
                Code::Api,
                format!("[DESIGN] a vertex of {} values for {} columns", vertex.0.len(), vertex_types.len()),
            ));
        }
        Ok(anchored(&vertex.0, vertex_types))
    }

    /// Calls `each` with the other end's key bytes and the row of each
    /// entry of the tree under `vertex` in `direction`: of its list, if
    /// kept, else of every entry the other way, those reaching it.
    ///
    /// # Errors
    ///
    /// As `anchored`; the tree's; `each`'s.
    fn reaching(&self, vertex: &Key, direction: Direction, each: &mut dyn FnMut(&[u8], RowId) -> Result<()>) -> Result<()> {
        let Some((prefix, checked)) = self.anchored(vertex, direction)? else { return Ok(()) };
        let vertex_types = self.layout.ends(direction).0;
        let kept = self.layout.kept.keeps(direction);
        let other = self.layout.kept.directions().first().copied().unwrap_or(Direction::Out);
        let start = if kept { tagged(tag(direction), &prefix) } else { vec![tag(other)] };
        for entry in scan(&self.tree, &start) {
            let (key, _) = entry?;
            let parsed = parse(&key, &self.layout)?;
            // Kept the other way, the entry's other end is the vertex.
            let (under, reached) = if kept { (parsed.vertex, parsed.other) } else { (parsed.other, parsed.vertex) };
            if (!kept || checked) && !equal(&decode_all(under, vertex_types)?, &vertex.0) {
                continue;
            }
            each(reached, parsed.row)?;
        }
        Ok(())
    }

    /// `Adjacency::degree`: the entries under `vertex` in `direction`, of
    /// its compacted adjacency or its degree's key if kept, else counted.
    ///
    /// # Errors
    ///
    /// As `steps_of`.
    pub(super) fn degree_of(&self, vertex: &Key, direction: Direction) -> Result<u64> {
        if self.layout.kept.keeps(direction)
            && let Some((prefix, false)) = self.anchored(vertex, direction)?
        {
            return self.listed(direction, &prefix);
        }
        let mut n = 0_u64;
        self.reaching(vertex, direction, &mut |_, _| {
            n = n.saturating_add(1);
            Ok(())
        })?;
        Ok(n)
    }
}
