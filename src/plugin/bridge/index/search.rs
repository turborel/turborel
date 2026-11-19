//! \[EXTENSION\] A plug-in method's index (RULES.md D226): its writes
//! kept and given a batch at a time, its searches read a page at a time,
//! each entry checked against the probe it answers.

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::sync::Arc;

use super::Shared;
use crate::error::Result;
use crate::index::{Entries, Index, Key, KeyOrder, NearestQuery, NeighbourEntries, Probe, RowFilter};
use crate::plugin::errors::violation;
use crate::plugin::payload::index::{Near, Probed, Write, entries, neighbours, put_near, put_probe};
use crate::plugin::payload::{INDEX_DROP, INDEX_NEAREST, INDEX_ROWS, INDEX_SEARCH};
use crate::plugin::values::put_values;
use crate::storage::RowId;
use crate::value::{Value, compare};

use super::super::answer;

/// A plug-in method's index.
#[derive(Debug)]
pub struct PluginIndex(pub(super) Arc<Shared>);

/// What SEARCH carries of `p`, if it carries it.
fn probed(p: &Probe) -> Option<Probed> {
    Some(match p {
        Probe::All => Probed::All,
        Probe::Compare { op, value } => Probed::Compare(*op, value.clone()),
        Probe::Prefix { equal, then } => Probed::Prefix(equal.clone(), then.clone()),
        Probe::Range { equal, low, high } => Probed::Range(equal.clone(), (**low).clone(), (**high).clone()),
        Probe::Columns(_) | Probe::Predicate { .. } | Probe::Xml(_) => return None,
    })
}

/// How key `a` compares with key `b`, column by column.
fn key_order(a: &[Value], b: &[Value]) -> Ordering {
    a.iter().zip(b).map(|(x, y)| compare(x, y).unwrap_or(Ordering::Equal)).find(|o| o.is_ne()).unwrap_or(Ordering::Equal)
}

/// A search, a page at a time.
struct Search {
    shared: Arc<Shared>,
    probe: Probe,
    probed: Probed,
    descending: bool,
    cursor: Vec<u8>,
    page: VecDeque<(RowId, Vec<Value>)>,
    done: bool,
    last: Option<Vec<Value>>,
}

impl Search {
    /// The next page read, if the page is empty and more follow.
    fn fill(&mut self) -> Result<()> {
        if !self.page.is_empty() || self.done {
            return Ok(());
        }
        let mut payload = Vec::new();
        crate::bytes::Put::put_bytes(&mut payload, &self.shared.locator);
        put_probe(&mut payload, &self.probed)?;
        crate::bytes::Put::put_u8(&mut payload, u8::from(self.descending));
        crate::bytes::Put::put_bytes(&mut payload, &self.cursor);
        crate::bytes::Put::put_u32(&mut payload, u32::try_from(self.shared.batch).unwrap_or(u32::MAX));
        let (got, cursor) = answer(&self.shared.link.plain(INDEX_SEARCH, payload)?, |r| Ok((entries(r)?, r.bytes()?.to_vec())))?;
        self.done = cursor.is_empty() || got.is_empty();
        self.cursor = cursor;
        self.page.extend(got);
        Ok(())
    }

    /// The next entry the probe selects, its order checked.
    fn step(&mut self) -> Result<Option<(Key, RowId)>> {
        loop {
            self.fill()?;
            let Some((id, key)) = self.page.pop_front() else { return Ok(None) };
            if self.shared.ordered
                && let Some(last) = &self.last
            {
                let wrong = if self.descending { Ordering::Less } else { Ordering::Greater };
                if key_order(last, &key) == wrong {
                    return Err(violation("its SEARCH gave entries out of order"));
                }
            }
            self.last = Some(key.clone());
            if self.probe.answers(&key) {
                return Ok(Some((Key(key), id)));
            }
        }
    }
}

impl PluginIndex {
    /// The ids of the entries of `key`, every write given first.
    fn rows_of(&self, key: &Key) -> Result<Vec<RowId>> {
        self.0.give()?;
        let mut payload = Vec::new();
        crate::bytes::Put::put_bytes(&mut payload, &self.0.locator);
        put_values(&mut payload, &key.0)?;
        let got = answer(&self.0.link.plain(INDEX_ROWS, payload)?, |r| (0..r.u32()?).map(|_| r.u64()).collect::<Result<Vec<_>>>())?;
        Ok(got)
    }

    /// The nearest entries `query` asks, at most `first`.
    fn near(&self, query: &NearestQuery, first: usize) -> Result<Option<NeighbourEntries>> {
        let Some(metric) = query.metric.clone() else { return Ok(None) };
        self.0.give()?;
        let n = Near {
            column: u32::try_from(query.column).unwrap_or(u32::MAX),
            value: query.value.clone(),
            metric,
            first: u32::try_from(first).unwrap_or(u32::MAX),
            descending: query.descending,
        };
        let mut payload = Vec::new();
        put_near(&mut payload, &self.0.locator, &n)?;
        let got = answer(&self.0.link.plain(INDEX_NEAREST, payload)?, neighbours)?;
        Ok(Some(Box::new(got.into_iter().map(|(d, id, key)| Ok((d, Key(key), id))))))
    }
}

impl Index for PluginIndex {
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.0.write(Write { insert: true, id: row, key: key.0 })
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.0.write(Write { insert: false, id: row, key: key.0.clone() })
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        self.rows_of(key)
    }
    fn search(self: Arc<Self>, probe: &Probe, order: KeyOrder) -> Entries {
        let Some(probed) = probed(probe) else {
            return Box::new(std::iter::once(Err(violation("an index of a plug-in's method was asked a probe it does not answer"))));
        };
        if let Err(e) = self.0.give() {
            return Box::new(std::iter::once(Err(e)));
        }
        let descending = order == KeyOrder::Descending;
        let search = Search {
            shared: Arc::clone(&self.0),
            probe: probe.clone(),
            probed,
            descending,
            cursor: Vec::new(),
            page: VecDeque::new(),
            done: false,
            last: None,
        };
        let mut search = Some(search);
        Box::new(std::iter::from_fn(move || {
            let s = search.as_mut()?;
            let got = s.step().transpose();
            if !matches!(got, Some(Ok(_))) {
                search = None;
            }
            got
        }))
    }
    fn estimate(&self, _probe: &Probe) -> Option<usize> {
        None
    }
    fn distinct_keys(&self) -> Option<f64> {
        self.0.known(|i| i.distinct)
    }
    fn probe_pages(&self, _probe: &Probe) -> Option<f64> {
        self.0.known(|i| i.pages)
    }
    fn nearest(self: Arc<Self>, _query: &NearestQuery, _budget: &Arc<crate::memory::MemoryBudget>) -> Result<Option<NeighbourEntries>> {
        Ok(None)
    }
    fn approximate(
        self: Arc<Self>,
        query: &NearestQuery,
        _budget: &Arc<crate::memory::MemoryBudget>,
        _filter: Option<&mut dyn RowFilter>,
    ) -> Result<Option<NeighbourEntries>> {
        let Some(asked) = &query.approximate else { return Ok(None) };
        let widen = usize::try_from(asked.widen).unwrap_or(1).max(1);
        self.near(query, asked.first.max(1).saturating_mul(widen))
    }
    fn locator(&self) -> Vec<u8> {
        self.0.locator.clone()
    }
    fn discard(&self) {
        let mut payload = Vec::new();
        crate::bytes::Put::put_bytes(&mut payload, &self.0.locator);
        // An index dropped whose program fails to drop it leaves its
        // entries in the program's care, read by nothing.
        drop(self.0.link.plain(INDEX_DROP, payload));
    }
}
