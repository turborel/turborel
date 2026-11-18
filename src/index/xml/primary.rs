//! \[EXTENSION\] RULES.md D106: the primary XML index's answers, from its
//! entries alone. It keeps each element and attribute of a row's value
//! twice: as a row of the Infoset of Pal et al. [§3.1.1], keyed by base
//! row and ORDPATH, with its kind, its path's id and its string value,
//! whole up to `PRIMARY_VALUE_BYTES`; and by row, path and value, as
//! their PROPERTY index keeps it [§3.2.2], so a row's nodes of a path and
//! value are found by a seek.
//!
//! The nodes a query's path selects of a row are read by path and value:
//! of each path of the dictionary its steps match, the row's entries of
//! that path, of the values its first string comparison may hold of; then
//! put in ORDPATH order, document order, an element before its attributes
//! and they before its children [§2.2]. A filter's anchor is the node
//! `above` steps above a node selected: its ORDPATH less as many
//! components, as §3.1.2's Parent joins a node to its parent. The nodes
//! below the anchor are its rows of the Infoset whose ORDPATH begins with
//! its own, read as one range [§2.2]; the filter selects those whose
//! names below the anchor its steps select. The first node, in document
//! order, that holds or may answers, as `XmlQuery::matches` answers.
//!
//! An entry is of a value's version, and a row whose versions held hold
//! values of more than one identity has the entries of each: what its
//! entries answer then only says the row may hold, and it is checked. So
//! is a row whose answer needs a value cut, or raises.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicUsize};

use super::check::compared_by;
use super::dictionary::tokens_of;
use super::keys::{Cursor, ENTRIES, PATHED, STRING, VERSIONS, after, corrupt, ordinal_of, pathed_fields, primary_fields, text_of};
use super::lookup::{
    Comparison, Literal, Resolved, ValueRange, XmlAnswer, XmlAnswers, XmlCost, XmlFilter, XmlLookup, XmlQuery, value_ranges,
};
use super::search::matched_paths;
use super::{Held, PRIMARY_VALUE_BYTES, XmlIndex};
use crate::engine::tree::{Direction, scan_of};
use crate::error::Result;
use crate::storage::RowId;

/// \[DESIGN\] The most entries a cost counts, as the other methods' do.
const MOST_COUNTED: usize = 1_000;
/// \[DESIGN\] The entries a page holds, about, of a node of a short value,
/// for the pages a cost counts: no measurement, a page's octets over an
/// entry's of a few names deep.
const ENTRIES_A_PAGE: usize = 64;

/// What a node, or a filter, answers: false; true; or maybe, of a value
/// cut, or of a comparison that raises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    False,
    True,
    Maybe,
}

/// A node of a row: its ORDPATH's ordinals, its path's names from below
/// its item, its value and whether it is whole.
struct Shredded {
    ordpath: Vec<u8>,
    names: Arc<Vec<u64>>,
    text: Vec<u8>,
    whole: bool,
}

/// A query resolved: the paths its lookup selects, each with its names;
/// the ranges of values its first string comparison may hold of; and its
/// filter's steps above and path, none if a name it tests has no token.
struct Plan {
    paths: Vec<(u64, Arc<Vec<u64>>)>,
    values: Option<Vec<ValueRange>>,
    filter: Option<(usize, Option<Resolved>)>,
}

/// The names of path `path` from below its item down, by the dictionary.
fn names_of(index: &XmlIndex, path: u64) -> Result<Arc<Vec<u64>>> {
    let (_, mut tokens) = tokens_of(&index.dictionary().path_of(path)?.ok_or_else(corrupt)?)?;
    tokens.reverse();
    Ok(Arc::new(tokens))
}

/// The octets of `ordpath`'s first components, all but its last `less`.
fn ancestor(ordpath: &[u8], less: usize) -> Result<&[u8]> {
    let mut ends = Vec::new();
    let mut at = Cursor { bytes: ordpath, at: 0 };
    while at.at < ordpath.len() {
        at.ordinal()?;
        ends.push(at.at);
    }
    let kept = ends.len().checked_sub(less).and_then(|n| n.checked_sub(1)).and_then(|n| ends.get(n)).copied();
    kept.and_then(|end| ordpath.get(..end)).ok_or_else(corrupt)
}

/// What `comparisons` answer of `node`'s value, in order, as
/// `compared_by` does of a whole one: of a value cut, a string compared
/// by its first octets where they differ from the literal's, else maybe;
/// a number, whose cast needs the whole, maybe, as it may raise.
fn compared(comparisons: &[Comparison], node: &Shredded) -> Outcome {
    if node.whole {
        return match std::str::from_utf8(&node.text).map(|t| compared_by(comparisons, t)) {
            Ok(Ok(true)) => Outcome::True,
            Ok(Ok(false)) => Outcome::False,
            Ok(Err(_)) | Err(_) => Outcome::Maybe,
        };
    }
    let mut outcome = Outcome::True;
    for comparison in comparisons {
        let order = match &comparison.literal {
            Literal::String(s) | Literal::Untyped(s) => cut_order(&node.text, s.as_bytes()),
            Literal::Number(_) => return Outcome::Maybe,
        };
        match order.map(|o| comparison.op.holds(o)) {
            Some(true) => {}
            Some(false) => return Outcome::False,
            None => outcome = Outcome::Maybe,
        }
    }
    outcome
}

/// How a string whose first octets are `prefix`, and that has more,
/// compares with `literal` by code points [XQueryFO §7.3.1], if they
/// tell: where they differ within the prefix, or the literal is no longer
/// than it and begins it, so is less.
fn cut_order(prefix: &[u8], literal: &[u8]) -> Option<Ordering> {
    let shared = prefix.iter().zip(literal).map(|(a, b)| a.cmp(b)).find(|o| o.is_ne());
    shared.or_else(|| (literal.len() <= prefix.len()).then_some(Ordering::Greater))
}

impl XmlIndex {
    /// The rows of the primary's entries `query` holds of, or may, each
    /// once, in row order: of one row if its lookup asks of one.
    pub(super) fn primary_answers(self: &Arc<Self>, query: &XmlQuery) -> XmlAnswers {
        self.answering(query, Arc::default())
    }

    /// `primary_answers`, the entries it reads counted in `read`.
    fn answering(self: &Arc<Self>, query: &XmlQuery, read: Arc<AtomicUsize>) -> XmlAnswers {
        let plan = match self.plan(query, &read) {
            Ok(Some(plan)) => Arc::new(plan),
            Ok(None) => return Box::new(std::iter::empty()),
            Err(e) => return Box::new(std::iter::once(Err(e))),
        };
        let (index, query) = (Arc::clone(self), query.clone());
        let answers = self.rows(query.lookup.row).filter_map(move |row| {
            let answering = Answering { index: &index, query: &query, plan: &plan, read: &read };
            match row.and_then(|r| answering.answer(r).map(|a| (r, a))) {
                Ok((row, Some(answer))) => Some(Ok((row, answer))),
                Ok((_, None)) => None,
                Err(e) => Some(Err(e)),
            }
        });
        Box::new(answers)
    }

    /// The rows the primary holds a value of, each once, in order, or
    /// `row` alone.
    fn rows(self: &Arc<Self>, row: Option<RowId>) -> Box<dyn Iterator<Item = Result<RowId>> + Send> {
        if let Some(row) = row {
            return Box::new(std::iter::once(Ok(row)));
        }
        let versions =
            scan_of(Held(Arc::clone(self)), Bound::Included(&[VERSIONS][..]), Bound::Excluded(&[PATHED][..]), Direction::Forward);
        let mut last = None;
        Box::new(versions.filter_map(move |found| {
            let next = found.and_then(|(key, _)| Cursor { bytes: &key, at: 1 }.row()).map(|r| (last.replace(r) != Some(r)).then_some(r));
            next.transpose()
        }))
    }

    /// The query resolved by the dictionary: none if a name its path
    /// tests has no token, so no node has its path.
    fn plan(self: &Arc<Self>, query: &XmlQuery, read: &AtomicUsize) -> Result<Option<Plan>> {
        let dictionary = self.dictionary();
        let filter = match &query.filter {
            Some(filter) => {
                query.lookup.anchored(filter)?;
                Some((filter.above, filter.pattern.resolve(&dictionary)?))
            }
            None => None,
        };
        let Some(path) = query.lookup.pattern.resolve(&dictionary)? else { return Ok(None) };
        let mut paths = Vec::new();
        for id in matched_paths(self, Arc::new(path), read) {
            let id = id?;
            paths.push((id, names_of(self, id)?));
        }
        let values =
            value_ranges(&query.lookup.comparisons, PRIMARY_VALUE_BYTES).filter(|r| r.iter().all(|v| v.low.first() == Some(&STRING)));
        Ok(Some(Plan { paths, values, filter }))
    }

    /// Whether `row` has entries of values of more than one identity.
    fn versions(&self, row: RowId) -> Result<bool> {
        let prefix = [&[VERSIONS][..], &row.to_be_bytes()].concat();
        let upper = after(&prefix);
        let held = self.tree.scan(Bound::Included(&prefix), upper.as_ref().map(Vec::as_slice), Direction::Forward);
        Ok(held.take(2).collect::<Result<Vec<_>>>()?.len() > 1)
    }

    /// What the primary's lookup costs: the entries it reads, counted to
    /// `MOST_COUNTED`, and a descent and the pages they fill.
    pub(super) fn primary_cost(self: &Arc<Self>, lookup: &XmlLookup) -> XmlCost {
        let read = Arc::new(AtomicUsize::new(0));
        let query = XmlQuery { lookup: lookup.clone(), filter: None };
        for answer in self.answering(&query, Arc::clone(&read)) {
            if answer.is_err() || read.load(atomic::Ordering::Relaxed) > MOST_COUNTED {
                return XmlCost { entries: None, pages: None };
            }
        }
        let entries = read.load(atomic::Ordering::Relaxed);
        let height = self.tree.height().ok().map(f64::from);
        let filled = u32::try_from(entries.div_ceil(ENTRIES_A_PAGE)).map_or(f64::from(u32::MAX), f64::from);
        XmlCost { entries: Some(entries), pages: height.map(|h| h + filled) }
    }
}

/// The answer of one row.
struct Answering<'a> {
    index: &'a Arc<XmlIndex>,
    query: &'a XmlQuery,
    plan: &'a Plan,
    /// The entries read.
    read: &'a AtomicUsize,
}

impl Answering<'_> {
    /// What the query answers of `row`: none if it does not hold.
    fn answer(&self, row: RowId) -> Result<Option<XmlAnswer>> {
        let mut nodes = Vec::new();
        for (path, names) in &self.plan.paths {
            self.selected(row, (*path, names), &mut nodes)?;
        }
        nodes.sort_by(|a, b| a.ordpath.cmp(&b.ordpath));
        let mut anchors: HashMap<Vec<u8>, Outcome> = HashMap::new();
        for node in &nodes {
            let mut outcome = compared(&self.query.lookup.comparisons, node);
            if let (Outcome::True, Some((above, path)), Some(filter)) = (outcome, &self.plan.filter, &self.query.filter) {
                let anchor = ancestor(&node.ordpath, *above)?;
                outcome = if let Some(&known) = anchors.get(anchor) {
                    known
                } else {
                    let depth = node.names.len().saturating_sub(*above);
                    let known = path.as_ref().map_or(Ok(Outcome::False), |p| self.filtered(row, (filter, p), (anchor, depth)))?;
                    anchors.insert(anchor.to_vec(), known);
                    known
                };
            }
            match outcome {
                Outcome::False => {}
                Outcome::Maybe => return Ok(Some(XmlAnswer::Check)),
                Outcome::True => return Ok(Some(if self.index.versions(row)? { XmlAnswer::Check } else { XmlAnswer::Holds })),
            }
        }
        Ok(None)
    }

    /// Adds to `nodes` the nodes of `row` of path `path`, of `names`, of
    /// the values the query's first string comparison may hold of.
    fn selected(&self, row: RowId, (path, names): (u64, &Arc<Vec<u64>>), nodes: &mut Vec<Shredded>) -> Result<()> {
        let prefix = [&[PATHED][..], &row.to_be_bytes(), &ordinal_of(path)].concat();
        let ranges: Vec<(Vec<u8>, Bound<Vec<u8>>)> = match &self.plan.values {
            Some(values) => {
                values.iter().map(|r| ([&prefix[..], &r.low].concat(), Bound::Excluded([&prefix[..], &r.high].concat()))).collect()
            }
            None => vec![(prefix.clone(), after(&prefix))],
        };
        for (low, high) in ranges {
            for found in self.index.tree.scan(Bound::Included(&low), high.as_ref().map(Vec::as_slice), Direction::Forward) {
                let (key, _) = found?;
                self.read.fetch_add(1, atomic::Ordering::Relaxed);
                let (value, ordpath) = pathed_fields(&key, prefix.len())?;
                let (text, whole) = text_of(value)?;
                nodes.push(Shredded { ordpath: ordpath.to_vec(), names: Arc::clone(names), text, whole });
            }
        }
        Ok(())
    }

    /// What `filter`, of path `path`, answers of its anchor of ORDPATH
    /// `anchor`, `depth` names below its item, in `row`: of its rows of the
    /// Infoset below it, one range of ORDPATHs [Pal et al. §2.2], those of
    /// names below it the filter's steps select; true if one holds, else
    /// maybe if one may, else false.
    fn filtered(&self, row: RowId, (filter, path): (&XmlFilter, &Resolved), (anchor, depth): (&[u8], usize)) -> Result<Outcome> {
        let prefix = [&[ENTRIES][..], &row.to_be_bytes(), anchor].concat();
        // Below it, its ORDPATH and more ordinals, each of a length of 1 or
        // more: past its own entry, whose ORDPATH ends.
        let (below, upper) = ([&prefix[..], &[1]].concat(), after(&prefix));
        let mut outcome = Outcome::False;
        let mut names: HashMap<u64, Arc<Vec<u64>>> = HashMap::new();
        for found in self.index.tree.scan(Bound::Included(&below), upper.as_ref().map(Vec::as_slice), Direction::Forward) {
            let (key, _) = found?;
            self.read.fetch_add(1, atomic::Ordering::Relaxed);
            let fields = primary_fields(&mut Cursor { bytes: &key, at: 1 })?;
            let known = if let Some(known) = names.get(&fields.path) {
                Arc::clone(known)
            } else {
                let known = names_of(self.index, fields.path)?;
                names.insert(fields.path, Arc::clone(&known));
                known
            };
            if !path.selects(fields.kind, known.get(depth..).unwrap_or_default()) {
                continue;
            }
            let (text, whole) = text_of(fields.value)?;
            match compared(&filter.comparisons, &Shredded { ordpath: Vec::new(), names: known, text, whole }) {
                Outcome::True => return Ok(Outcome::True),
                Outcome::Maybe => outcome = Outcome::Maybe,
                Outcome::False => {}
            }
        }
        Ok(outcome)
    }
}

// Unit tests: the primary's answers are a value's own, of many values
// and queries made, and its parent join and ORDPATH tests (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
