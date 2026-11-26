//! \[DESIGN\] Column sketches of a frozen group's texts coded by symbols
//! (RULES.md D525), as Hentschel, Kester and Idreos, "Column Sketches: A
//! Scan Accelerator for Rapid and Robust Predicate Evaluation" (SIGMOD
//! 2018), describe them: of a column of VARCHAR that COLUMNSTORE.SKETCH
//! names, whose chunks are coded by a table of symbols (`fsst.rs`), a
//! byte of each row, its code, from an equi-depth histogram of the
//! group's values, so codes order as the values do. The histogram's
//! edges are drawn from a sorted sample; a value found often among it is
//! an edge with a code of its own, a point, so a condition of it is told
//! by codes alone, and each other code holds the values between two
//! edges. A null's code is `NULL_CODE`.
//!
//! A condition of the column is judged of each code once
//! (`judged/sketched.rs`): true of every value the code holds, of none,
//! or of some, the code then a boundary. A scan keeps or leaves a row by
//! its code alone, and compares with the constants only the rows of a
//! boundary code, so of the 255 codes at most one a constant reads its
//! text for.
//!
//! The codes of a part of `PART_ROWS` rows are kept together, those of
//! as many parts as a page holds on one, read only by a scan whose
//! condition can use them; the edges in the group's header.

use std::cmp::Ordering;

use super::encoded::FSST;
use super::group::{Filter, Group, Pages, Seal, Zone};
use super::imprints::parts_of;
use super::padding;
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::value::{DataType, Value};

/// \[DESIGN\] The code of a null, past every value's.
pub const NULL_CODE: u8 = u8::MAX;
/// \[DESIGN\] The most codes of values: every byte but the null's.
const MOST_CODES: usize = 255;
/// \[DESIGN\] The most values the edges are drawn from, taken evenly
/// apart, as imprints draw theirs (`imprints.rs`).
const SAMPLE: usize = 4096;
/// \[DESIGN\] The most values given codes of their own for being
/// frequent: each takes two codes, its own and the one below it, so the
/// rest keep at least half of them.
const MOST_POINTS: usize = 64;
/// The byte a string is padded with as it is compared: a space.
const PAD: u8 = b' ';

/// \[DESIGN\] The order of the texts `a` and `b`, each padded with spaces
/// to the other's length, as `compare_strings` orders them: UTF-8
/// orders as the characters it encodes, and a space is one byte, so
/// their bytes order them, the longer's past the shorter's against
/// spaces.
pub fn padded_order(a: &[u8], b: &[u8]) -> Ordering {
    let shared = a.len().min(b.len());
    let (head_a, rest_a) = a.split_at(shared);
    let (head_b, rest_b) = b.split_at(shared);
    match head_a.cmp(head_b) {
        Ordering::Equal => {
            let beyond = |rest: &[u8]| rest.iter().find(|&&k| k != PAD).map_or(Ordering::Equal, |&k| k.cmp(&PAD));
            if rest_a.is_empty() { beyond(rest_b).reverse() } else { beyond(rest_a) }
        }
        other @ (Ordering::Less | Ordering::Greater) => other,
    }
}

/// What the values of a code are, by the edges: those between edge
/// `edge` and the one below it, that edge's own among them if
/// `inclusive`; those equal to edge `edge`, a point; or those above
/// every edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bin {
    /// Above the edge below `edge`, up to `edge`, it too if `inclusive`.
    Below {
        /// The edge above.
        edge: usize,
        /// Whether it holds the edge above.
        inclusive: bool,
    },
    /// Equal to `edge`.
    Point(usize),
    /// Above the last edge, of so many edges.
    Above(usize),
}

impl Bin {
    /// The order of every value of the bin against a constant whose place
    /// among the edges is `lo`, the first not below it, equal to it if
    /// `equal`; none if its values order variously against it.
    pub fn order(self, (lo, equal): (usize, bool)) -> Option<Ordering> {
        match self {
            Bin::Point(edge) => Some(match edge.cmp(&lo) {
                Ordering::Less => Ordering::Less,
                Ordering::Equal if equal => Ordering::Equal,
                Ordering::Equal | Ordering::Greater => Ordering::Greater,
            }),
            Bin::Below { edge, inclusive } => match edge.cmp(&lo) {
                Ordering::Less => Some(Ordering::Less),
                Ordering::Greater => Some(Ordering::Greater),
                // Between the edges about the constant, of it only if it
                // is the edge and the bin does not hold it.
                Ordering::Equal => (equal && !inclusive).then_some(Ordering::Less),
            },
            // Above every edge, of a constant below one of them.
            Bin::Above(edges) => (lo < edges).then_some(Ordering::Greater),
        }
    }
}

/// \[DESIGN\] The edges of a column's sketch, ascending and apart as
/// strings compare, and of each whether it is a point; none if the column
/// has no sketch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edges {
    texts: Vec<String>,
    points: Vec<bool>,
    /// The code of the bin below each edge.
    below: Vec<u8>,
}

impl Edges {
    /// The edges `texts`, each a point if `points` says so: none if they
    /// are not apart and ascending, or need more codes than a byte holds.
    pub fn new(texts: Vec<String>, points: Vec<bool>) -> Option<Edges> {
        let apart = texts.iter().zip(texts.iter().skip(1)).all(|(a, b)| padded_order(a.as_bytes(), b.as_bytes()) == Ordering::Less);
        if !apart || points.len() != texts.len() {
            return None;
        }
        let mut below = Vec::with_capacity(texts.len());
        let mut code = 0_u8;
        for &point in &points {
            below.push(code);
            code = code.checked_add(if point { 2 } else { 1 })?;
        }
        (code < NULL_CODE).then_some(Edges { texts, points, below })
    }

    /// Whether it has no edges.
    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }

    /// Its edges, and of each whether it is a point.
    pub fn each(&self) -> impl Iterator<Item = (&str, bool)> {
        self.texts.iter().map(String::as_str).zip(self.points.iter().copied())
    }

    /// The place of `text` among its edges: the first not below it, and
    /// whether that one is equal to it.
    pub fn place(&self, text: &[u8]) -> (usize, bool) {
        let lo = self.texts.partition_point(|e| padded_order(e.as_bytes(), text) == Ordering::Less);
        (lo, self.texts.get(lo).is_some_and(|e| padded_order(e.as_bytes(), text) == Ordering::Equal))
    }

    /// The code of `text`.
    pub fn code(&self, text: &[u8]) -> u8 {
        let (lo, equal) = self.place(text);
        match (self.below.get(lo), self.points.get(lo)) {
            (Some(&below), Some(&point)) => below.saturating_add(u8::from(point && equal)),
            (None, _) | (_, None) => self.above(),
        }
    }

    /// The code of the values above every edge.
    fn above(&self) -> u8 {
        self.below.last().zip(self.points.last()).map_or(0, |(&b, &p)| b.saturating_add(if p { 2 } else { 1 }))
    }

    /// Each code of a value, and its bin.
    pub fn bins(&self) -> impl Iterator<Item = (u8, Bin)> + '_ {
        let each = self.below.iter().zip(&self.points).enumerate().flat_map(|(edge, (&below, &point))| {
            let first = (below, Bin::Below { edge, inclusive: !point });
            let own = point.then(|| (below.saturating_add(1), Bin::Point(edge)));
            std::iter::once(first).chain(own)
        });
        each.chain(std::iter::once((self.above(), Bin::Above(self.texts.len()))))
    }

    /// The edges of `values`, of a column of texts (RULES.md D525): a
    /// sorted sample of them cut into bins of about as many each, and the
    /// most frequent, each among at least a bin's share, points.
    pub fn of(values: &[&Value]) -> Edges {
        let step = values.len().div_ceil(SAMPLE).max(1);
        let mut sample: Vec<&[u8]> =
            values.iter().step_by(step).filter_map(|v| if let Value::Char(s) = v { Some(s.as_bytes()) } else { None }).collect();
        sample.sort_by(|a, b| padded_order(a, b));
        // Each run of one value: where it begins and how many.
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for (n, text) in sample.iter().enumerate() {
            let same = |run: &&mut (usize, usize)| sample.get(run.0).is_some_and(|first| padded_order(first, text) == Ordering::Equal);
            if let Some(run) = runs.last_mut().filter(same) {
                run.1 = run.1.saturating_add(1);
            } else {
                runs.push((n, 1));
            }
        }
        let few = runs.len() <= MOST_CODES.saturating_sub(1) >> 1_u32;
        let share = sample.len().checked_div(MOST_CODES.saturating_sub(1)).unwrap_or(0).max(2);
        let mut frequent: Vec<(usize, usize)> = runs.iter().copied().filter(|&(_, count)| few || count >= share).collect();
        frequent.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
        frequent.truncate(if few { MOST_CODES } else { MOST_POINTS });
        let bins = MOST_CODES.saturating_sub(frequent.len().saturating_mul(2));
        let mut chosen: Vec<(usize, bool)> = frequent.iter().map(|&(at, _)| (at, true)).collect();
        if !few {
            chosen.extend((1..bins).filter_map(|n| n.saturating_mul(sample.len()).checked_div(bins)).map(|at| (at, false)));
        }
        Self::chosen(&sample, &runs, chosen)
    }

    /// The edges of `sample`, sorted, whose runs of one value are `runs`,
    /// at the places `chosen`, each a point or not: each place taken as
    /// its run's first, a run chosen twice a point if either is.
    fn chosen(sample: &[&[u8]], runs: &[(usize, usize)], mut chosen: Vec<(usize, bool)>) -> Edges {
        for c in &mut chosen {
            c.0 = runs.get(runs.partition_point(|&(first, _)| first <= c.0).saturating_sub(1)).map_or(c.0, |r| r.0);
        }
        chosen.sort_unstable();
        chosen.dedup_by(|later, kept| {
            let same = later.0 == kept.0;
            kept.1 |= same && later.1;
            same
        });
        // Each sampled text was a string, so is UTF-8.
        let (texts, points): (Vec<String>, Vec<bool>) =
            chosen.iter().filter_map(|&(at, point)| Some((std::str::from_utf8(sample.get(at)?).ok()?.to_owned(), point))).unzip();
        Edges::new(texts, points).unwrap_or_default()
    }

    /// About the bytes it takes in memory.
    pub fn bytes(&self) -> usize {
        self.texts.iter().map(String::capacity).sum::<usize>().saturating_add(self.points.len()).saturating_add(self.below.len())
    }
}

/// Whether a column of type `t` may get a sketch: VARCHAR, whose texts
/// alone a table of symbols codes.
pub fn sketched(t: &DataType) -> bool {
    padding(t) == Some(None)
}

/// Writes the sketch of each column of `types` at `places` that takes
/// one (`sketched`) and has a chunk coded by symbols, of `rows`, in its
/// chunks `columns`, if a page holds a part's codes: of each column, its
/// edges, none if it has none.
///
/// # Errors
///
/// As `Pages::put`.
pub(super) fn put_all<S: Seal, R: AsRef<[Value]>>(
    pages: &Pages<'_, S>,
    (types, rows, places): (&[DataType], &[R], &[usize]),
    columns: &mut [Vec<Zone>],
    written: &mut Vec<PageId>,
) -> Result<Vec<Edges>> {
    let mut out = vec![Edges::default(); types.len()];
    let part = usize::try_from(super::group::PART_ROWS).unwrap_or(usize::MAX);
    for &c in places {
        let (Some(t), Some(zones), Some(slot)) = (types.get(c), columns.get_mut(c), out.get_mut(c)) else { continue };
        if !sketched(t) || pages.capacity < part || !coded(pages, zones)? {
            continue;
        }
        let values: Vec<&Value> = rows.iter().map(|r| r.as_ref().get(c).unwrap_or(&Value::Null)).collect();
        let edges = Edges::of(&values);
        if edges.is_empty() {
            continue;
        }
        put_codes(pages, (&edges, &values), zones, written)?;
        *slot = edges;
    }
    Ok(out)
}

/// Whether one of the chunks `zones` is coded by symbols.
fn coded<S: Seal>(pages: &Pages<'_, S>, zones: &[Zone]) -> Result<bool> {
    for zone in zones {
        if pages.with_run(&zone.run, |bytes| Ok(bytes.first() == Some(&FSST)))? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The code of `v` by `edges`.
fn code_of(edges: &Edges, v: &Value) -> u8 {
    if let Value::Char(s) = v { edges.code(s.as_bytes()) } else { NULL_CODE }
}

/// Writes the codes of each part of the chunks `zones`, of the column
/// `values`, by `edges`, those of as many parts as a page holds on one,
/// and puts where each is in its part.
fn put_codes<S: Seal>(
    pages: &Pages<'_, S>,
    (edges, values): (&Edges, &[&Value]),
    zones: &mut [Zone],
    written: &mut Vec<PageId>,
) -> Result<()> {
    let mut places: Vec<Filter> = Vec::new();
    let (mut page, mut held): (Vec<u8>, Vec<(u32, u32)>) = (Vec::new(), Vec::new());
    for (from, to) in zones.iter().flat_map(parts_of) {
        let these = values.get(usize::try_from(from).unwrap_or(usize::MAX)..usize::try_from(to).unwrap_or(0)).unwrap_or_default();
        if !page.is_empty() && page.len().saturating_add(these.len()) > pages.capacity {
            put_page(pages, (&page, &mut held), &mut places, written)?;
            page.clear();
        }
        let at = u32::try_from(page.len()).unwrap_or(u32::MAX);
        page.extend(these.iter().map(|v| code_of(edges, v)));
        held.push((at, u32::try_from(these.len()).unwrap_or(u32::MAX)));
    }
    if !page.is_empty() {
        put_page(pages, (&page, &mut held), &mut places, written)?;
    }
    let mut places = places.into_iter();
    for part in zones.iter_mut().flat_map(|z| z.parts.iter_mut()) {
        part.sketch = places.next();
    }
    Ok(())
}

/// Writes `page`, the codes of the parts `held` says where each is on,
/// and pushes where each is to `places`.
fn put_page<S: Seal>(
    pages: &Pages<'_, S>,
    (page, held): (&[u8], &mut Vec<(u32, u32)>),
    places: &mut Vec<Filter>,
    written: &mut Vec<PageId>,
) -> Result<()> {
    let at = pages.put(page, written)?;
    places.extend(held.drain(..).map(|(from, len)| Filter { page: at, at: from, len }));
    Ok(())
}

/// \[DESIGN\] What a code says of a row, of a condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The condition is true of every value of the code.
    Keep,
    /// It is true of none.
    Drop,
    /// It may be true of some: the row's value is compared.
    Check,
}

/// \[DESIGN\] What each code says of a row, of a condition.
#[derive(Debug, Clone, Copy)]
pub struct Verdicts([Verdict; 256]);

impl Verdicts {
    /// Every code a boundary.
    pub fn checked() -> Verdicts {
        Verdicts([Verdict::Check; 256])
    }

    /// Code `code` says `verdict`.
    pub fn set(&mut self, code: u8, verdict: Verdict) {
        if let Some(slot) = self.0.get_mut(usize::from(code)) {
            *slot = verdict;
        }
    }

    /// What code `code` says.
    pub fn of(&self, code: u8) -> Verdict {
        self.0.get(usize::from(code)).copied().unwrap_or(Verdict::Check)
    }
}

/// The codes of a column of the group a scan reads, read as its runs ask
/// for them: of each part, in order, its rows and where its codes are.
struct Loaded {
    column: usize,
    codes: Vec<u8>,
    parts: Vec<((u32, u32), Option<Filter>)>,
    read: Vec<bool>,
}

/// \[DESIGN\] What a scan holds of the sketches of the group it reads: of
/// each column, the codes read, and of each conjunct, by its place, what
/// each code says of it, each made once a group.
#[derive(Default)]
pub struct Held {
    columns: Vec<Loaded>,
    verdicts: Vec<(usize, Option<Verdicts>)>,
}

impl Held {
    /// Lets everything go, as the next group is taken up.
    pub fn clear(&mut self) {
        self.columns.clear();
        self.verdicts.clear();
    }

    /// Whether it holds nothing of the group: no codes read, no code judged.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty() && self.verdicts.is_empty()
    }

    /// What each code says of the conjunct at `at`, made by `made` if not
    /// made before of this group.
    pub fn verdicts(&mut self, at: usize, made: impl FnOnce() -> Option<Verdicts>) -> Option<Verdicts> {
        if let Some((_, v)) = self.verdicts.iter().find(|(a, _)| *a == at) {
            return *v;
        }
        let v = made();
        self.verdicts.push((at, v));
        v
    }

    /// The codes of column `column` of the rows `from` to `to` of
    /// `group`, its pages read only once each; none if a part of them has
    /// no sketch.
    ///
    /// # Errors
    ///
    /// As `Pages::with`.
    pub fn codes<S: Seal>(
        &mut self,
        group: &Group,
        pages: &Pages<'_, S>,
        column: usize,
        (from, to): (u32, u32),
    ) -> Result<Option<Vec<u8>>> {
        let at = if let Some(at) = self.columns.iter().position(|l| l.column == column) {
            at
        } else {
            let Some(zones) = group.zones(column)? else { return Ok(None) };
            let parts: Vec<((u32, u32), Option<Filter>)> =
                zones.iter().flat_map(|z| parts_of(z).zip(z.parts.iter().map(|p| p.sketch))).collect();
            let read = vec![false; parts.len()];
            self.columns.push(Loaded { column, codes: vec![NULL_CODE; usize::try_from(group.rows).unwrap_or(0)], parts, read });
            self.columns.len().saturating_sub(1)
        };
        let Some(loaded) = self.columns.get_mut(at) else { return Ok(None) };
        let first = loaded.parts.partition_point(|&((_, end), _)| end <= from);
        for n in first..loaded.parts.len() {
            let Some(&((start, _), place)) = loaded.parts.get(n) else { break };
            if start >= to {
                break;
            }
            let Some(place) = place else { return Ok(None) };
            if !loaded.read.get(n).copied().unwrap_or(false) {
                loaded.load(pages, place.page)?;
            }
            // A part whose codes are not whole on their page has none.
            if !loaded.read.get(n).copied().unwrap_or(false) {
                return Ok(None);
            }
        }
        let range = usize::try_from(from).unwrap_or(usize::MAX)..usize::try_from(to).unwrap_or(0);
        Ok(loaded.codes.get(range).map(<[u8]>::to_vec))
    }
}

impl Loaded {
    /// Reads page `page`, copying the codes of every part on it.
    fn load<S: Seal>(&mut self, pages: &Pages<'_, S>, page: PageId) -> Result<()> {
        let Loaded { codes, parts, read, .. } = self;
        pages.with(page, |bytes| {
            for (((start, end), place), done) in parts.iter().zip(read.iter_mut()) {
                let Some(place) = place.filter(|p| p.page == page) else { continue };
                let at = usize::try_from(place.at).unwrap_or(usize::MAX);
                let len = usize::try_from(place.len).unwrap_or(0);
                let rows = usize::try_from(*start).unwrap_or(usize::MAX)..usize::try_from(*end).unwrap_or(0);
                let whole = (bytes.get(at..at.saturating_add(len)), codes.get_mut(rows));
                if let (Some(src), Some(dst)) = whole
                    && src.len() == dst.len()
                {
                    dst.copy_from_slice(src);
                    *done = true;
                }
            }
            Ok(())
        })?;
        Ok(())
    }
}

// Unit tests: the order of padded texts, edges and codes, and that a
// scan judging by sketches keeps what the values keep (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
