//! \[EXTENSION\] RULES.md D106: an XML index's lookups, read as they are
//! taken. The paths a lookup's pattern may match are found in the
//! dictionary by the tokens its last steps fix, each matched step by step
//! (`lookup.rs`); then, of `xml_path`, each matching path's entries in
//! each range of values; of `xml_value`, each range's entries, their paths
//! matched; of `xml_property`, of a row asked, each matching path's
//! entries of that row, or every entry, each checked, of none. A row is
//! given once (`Index::xml_rows`), a set of those given spilling to a work
//! area past `ONCE_BYTES` (RULES.md D38).

use std::collections::HashMap;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::keys::{ENTRIES, ROW, after, ordinal_of, parse_entry};
use super::lookup::{Resolved, ValueRange, XmlCost, XmlLookup, XmlQuery, value_ranges};
use super::{Held, Method, VALUE_PREFIX_BYTES, XmlIndex};
use crate::engine::tree::{Direction, Scan, scan_of};
use crate::error::Result;
use crate::index::RowIds;
use crate::spill::map::SpillMap;
use crate::spill::{DEFAULT_SPILL_BATCH, DEFAULT_WORK_MEMORY, ListSpace};
use crate::storage::RowId;
use crate::sync::lock_builtin;

/// \[DESIGN\] The bytes of the rows a lookup has given, held to give each
/// once, before they spill: an operator's own, by default.
const ONCE_BYTES: usize = DEFAULT_WORK_MEMORY;
/// \[DESIGN\] The most entries a cost counts: past them, it is not cheap,
/// as for a B-link tree's estimate (`index/blink.rs`).
const MOST_COUNTED: usize = 1_000;
/// \[DESIGN\] The most paths whose match a lookup remembers at once, so it
/// matches each once while memory holds them, and forgets them past it.
const MOST_REMEMBERED: usize = 4_096;

/// A lookup's rows, an entry's at a time, with each row as often as it
/// has entries.
type Found = Box<dyn Iterator<Item = Result<RowId>> + Send>;

/// The keys from `low` up to `high`.
fn scan(index: &Arc<XmlIndex>, low: &[u8], high: &Bound<Vec<u8>>, seeks: &AtomicUsize) -> Scan<Held> {
    seeks.fetch_add(1, Ordering::Relaxed);
    scan_of(Held(Arc::clone(index)), Bound::Included(low), high.as_ref().map(Vec::as_slice), Direction::Forward)
}

/// The keys of each range of `values`, or of every value if none, after
/// `prefix`, each with its key, and whether each must be checked against
/// the ranges.
fn ranged(index: &Arc<XmlIndex>, prefix: &[u8], values: Option<&[ValueRange]>, seeks: &Arc<AtomicUsize>) -> Vec<Scan<Held>> {
    match values {
        None => vec![scan(index, prefix, &after(prefix), seeks)],
        Some(ranges) => {
            ranges.iter().map(|r| scan(index, &[prefix, &r.low].concat(), &Bound::Excluded([prefix, &r.high].concat()), seeks)).collect()
        }
    }
}

/// The rows each entry of `scans` names, those of `row` alone if given,
/// of a path `paths` matches and a value in `values` if they are given.
fn rows_of(scans: Vec<Scan<Held>>, (method, row): (Method, Option<RowId>), checks: Checks) -> Found {
    let mut checks = checks;
    Box::new(scans.into_iter().flatten().filter_map(move |found| {
        let kept = found.and_then(|(key, _)| {
            let (path, value, at) = parse_entry(method, &key)?;
            let wanted = row.is_none_or(|r| r == at) && checks.values.as_ref().is_none_or(|v| v.iter().any(|r| r.holds(value)));
            Ok((wanted && checks.path(path)?).then_some(at))
        });
        kept.transpose()
    }))
}

/// What each entry read is checked by: its path matched, as remembered,
/// and its value in a range, if they are given.
struct Checks {
    paths: Option<(Arc<XmlIndex>, Arc<Resolved>)>,
    remembered: HashMap<u64, bool>,
    values: Option<Vec<ValueRange>>,
}

impl Checks {
    /// Whether path id `path` matches, if paths are checked.
    fn path(&mut self, path: u64) -> Result<bool> {
        let Some((index, resolved)) = &self.paths else { return Ok(true) };
        if let Some(&known) = self.remembered.get(&path) {
            return Ok(known);
        }
        let matched = match index.dictionary().path_of(path)? {
            Some(tokens) => resolved.matches(&tokens)?,
            None => false,
        };
        if self.remembered.len() >= MOST_REMEMBERED {
            self.remembered.clear();
        }
        self.remembered.insert(path, matched);
        Ok(matched)
    }
}

/// The ids of the paths of the dictionary `resolved` matches, as they
/// are read.
pub(super) fn matched_paths(
    index: &Arc<XmlIndex>,
    resolved: Arc<Resolved>,
    seeks: &AtomicUsize,
) -> Box<dyn Iterator<Item = Result<u64>> + Send> {
    if resolved.exact {
        seeks.fetch_add(1, Ordering::Relaxed);
        return Box::new(index.dictionary().path(&resolved.prefix, false).transpose().into_iter());
    }
    let from = [&[super::keys::PATHS][..], &resolved.prefix].concat();
    let paths = scan(index, &from, &after(&from), seeks);
    Box::new(paths.filter_map(move |found| {
        let matched = found.and_then(|(key, value)| {
            let path = key.get(1..).unwrap_or_default();
            resolved.matches(path)?.then(|| super::dictionary::id_of(&value)).transpose()
        });
        matched.transpose()
    }))
}

impl XmlIndex {
    /// The rows of each entry `lookup` reads, and the seeks it made, as
    /// they are taken: none if a name it tests has no token.
    pub(super) fn found(self: &Arc<Self>, lookup: &XmlLookup, seeks: &Arc<AtomicUsize>) -> Result<Found> {
        let Some(resolved) = lookup.pattern.resolve(&self.dictionary())? else { return Ok(Box::new(std::iter::empty())) };
        let resolved = Arc::new(resolved);
        let values = value_ranges(&lookup.comparisons, VALUE_PREFIX_BYTES);
        let by_path = |prefix: fn(u64, Option<RowId>) -> Vec<u8>| -> Found {
            let (index, seeks, values, row, method) = (Arc::clone(self), Arc::clone(seeks), values.clone(), lookup.row, self.method);
            Box::new(matched_paths(self, Arc::clone(&resolved), seeks.as_ref()).flat_map(move |path| match path {
                Ok(path) => {
                    let scans = ranged(&index, &prefix(path, row), values.as_deref(), &seeks);
                    rows_of(scans, (method, row), Checks { paths: None, remembered: HashMap::new(), values: None })
                }
                Err(e) => Box::new(std::iter::once(Err(e))),
            }))
        };
        Ok(match (self.method, lookup.row) {
            (Method::Path, _) => by_path(|path, _| [&[ENTRIES][..], &ordinal_of(path)].concat()),
            (Method::Property, Some(_)) => {
                by_path(|path, row| [&[ENTRIES][..], &row.unwrap_or_default().to_be_bytes(), &ordinal_of(path)].concat())
            }
            (Method::Value, _) => {
                let scans = ranged(self, &[ENTRIES], values.as_deref(), seeks);
                rows_of(scans, (self.method, lookup.row), self.checks(resolved, None))
            }
            (Method::Property, None) => {
                let scans = ranged(self, &[ENTRIES], None, seeks);
                rows_of(scans, (self.method, None), self.checks(resolved, values))
            }
            (Method::Primary, _) => {
                let query = XmlQuery { lookup: lookup.clone(), filter: None };
                Box::new(self.primary_answers(&query).map(|answer| answer.map(|(row, _)| row)))
            }
        })
    }

    /// The checks of entries read across paths: by `resolved`, and by
    /// `values` if given.
    fn checks(self: &Arc<Self>, resolved: Arc<Resolved>, values: Option<Vec<ValueRange>>) -> Checks {
        Checks { paths: Some((Arc::clone(self), resolved)), remembered: HashMap::new(), values }
    }

    /// The rows `lookup` may select, each once, as they are taken.
    pub(super) fn lookup_rows(self: Arc<Self>, lookup: &XmlLookup) -> RowIds {
        let found = match self.found(lookup, &Arc::default()) {
            Ok(found) => found,
            Err(e) => return Box::new(std::iter::once(Err(e))),
        };
        let areas = lock_builtin(&self.work).clone();
        let space = ListSpace { limit: ONCE_BYTES, batch: DEFAULT_SPILL_BATCH, areas, types: crate::value::types::TypeRegistry::default() };
        let mut given = SpillMap::new(space);
        Box::new(found.filter_map(move |row| {
            let first = row.and_then(|r| {
                let bytes: [u8; ROW] = r.to_be_bytes();
                if given.contains(&bytes)? {
                    return Ok(None);
                }
                given.insert(&bytes, &[])?;
                Ok(Some(r))
            });
            first.transpose()
        }))
    }

    /// What `lookup` costs: its entries counted to `MOST_COUNTED`, and its
    /// seeks, each its tree's height.
    pub(super) fn lookup_cost(self: &Arc<Self>, lookup: &XmlLookup) -> XmlCost {
        if self.method == Method::Primary {
            return self.primary_cost(lookup);
        }
        let seeks = Arc::new(AtomicUsize::new(0));
        let Ok(found) = self.found(lookup, &seeks) else { return XmlCost { entries: None, pages: None } };
        let mut entries = Some(0_usize);
        for row in found {
            if row.is_err() {
                return XmlCost { entries: None, pages: None };
            }
            entries = entries.map(|n| n.saturating_add(1)).filter(|&n| n <= MOST_COUNTED);
            if entries.is_none() {
                break;
            }
        }
        let height = self.tree.height().ok().map(f64::from);
        let seeks = u32::try_from(seeks.load(Ordering::Relaxed)).map_or(f64::from(u32::MAX), f64::from);
        XmlCost { entries, pages: height.map(|h| h * seeks) }
    }
}
