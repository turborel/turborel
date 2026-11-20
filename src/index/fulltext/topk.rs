//! \[DESIGN\] The documents of a full-text index in descending order of
//! their Rank for a pattern (RULES.md D86, D38) [ISO/IEC 13249-2:2002
//! (CD, SC32 N0640) §5.1.3], so FETCH FIRST n reads few postings.
//!
//! The rank of a document is its index's ranking's (`rank.rs`) over the
//! terms the pattern's words match, read from their blocks: each term's
//! frequency in the document, the document's length from its number
//! entry. Documents are given in batches: each batch the best `k` after
//! the last given, found by MaxScore (Turtle and Flood, 1995). A term's
//! most weight in any document is the ranking's bound
//! (`Ranking::bound`), of the greatest frequency its statistics keep; the
//! terms are sorted by it, and once `k` documents are held, the terms
//! whose bounds summed fall short of the least held are read only to
//! score a document another term found, not to find one. A document's
//! rank is summed as Rank sums it (`rank::score`), by the pattern's words
//! then their terms, so the two agree bit for bit. The statistics are one
//! snapshot, the statement's, so a rank never passes the bounds read of
//! it; a weight past its bound would make the order wrong, so it is
//! refused, naming the ranking (SQLCODE -910). Batches double,
//! so n documents cost the reading of the lists a few times over, and the
//! memory of the largest batch, reserved of the budget.
//!
//! Once the documents holding a term are given, every other document
//! follows, of rank 0, so every document is given once.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::Arc;

use super::FullTextIndex;
use super::cursor::{Cursor, Docs, Every, Terms};
use super::document::hash_key;
use super::keys::Doc;
use super::matcher::{Matcher, positive_matchers};
use super::rank::{broken, weighed};
use crate::error::Result;
use crate::fulltext::pattern::{contains, parse};
use crate::fulltext::rank::{Corpus, Ranking, Statistics, TermStats};
use crate::index::{Key, NeighbourEntries};
use crate::memory::{MemoryBudget, Reservation};
use crate::storage::RowId;
use crate::value::Value;

/// \[DESIGN\] The documents of the first batch.
const FIRST_BATCH: usize = 32;
/// \[DESIGN\] The most documents of a batch.
const MOST_BATCH: usize = 4_096;
/// \[DESIGN\] The bytes a document held in a batch is reserved for.
const HELD_BYTES: usize = 64;

/// A term of the pattern: its statistics, the most it adds to a
/// document's rank, and its place in the order Rank sums weights in.
#[derive(Debug, Clone)]
struct Feature {
    term: Vec<u8>,
    stats: TermStats,
    most: f64,
    order: usize,
}

/// A document ranked: its rank and number, ordered best first, the
/// lesser number first of equal rank.
#[derive(Debug, Clone, Copy)]
struct Ranked {
    rank: f64,
    docno: u64,
    doc: Doc,
}

/// Held in a heap whose top is the worst held.
impl Ord for Ranked {
    fn cmp(&self, other: &Ranked) -> Ordering {
        other.rank.total_cmp(&self.rank).then(self.docno.cmp(&other.docno))
    }
}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Ranked) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Ranked {
    fn eq(&self, other: &Ranked) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Ranked {}

/// What a batch reads.
struct Pass<'a> {
    index: &'a Arc<FullTextIndex>,
    features: &'a [Feature],
    ranking: &'a dyn Ranking,
    corpus: Corpus,
    /// The last document given: those of a batch come after it.
    after: Option<(f64, u64)>,
}

impl Pass<'_> {
    /// The best `k` documents after the last given, best first.
    fn best(&self, k: usize) -> Result<Vec<Ranked>> {
        let mut cursors: Vec<Cursor> = self.features.iter().map(|f| Cursor::new(self.index, f.term.clone())).collect();
        // bounds[i]: the most the features before and at i add.
        let bounds: Vec<f64> = self.features.iter().scan(0.0_f64, |sum, f| Some(*sum + f.most).inspect(|s| *sum = *s)).collect();
        let mut held: BinaryHeap<Ranked> = BinaryHeap::with_capacity(k.saturating_add(1));
        let mut essential = 0_usize;
        let mut next = 0_u64;
        while let Some(docno) = least_head(cursors.get_mut(essential..).unwrap_or_default(), next)? {
            next = docno.saturating_add(1);
            let Some((doc, length)) = self.index.numbered_with_length(docno)? else { continue };
            let threshold = if held.len() >= k { held.peek().map_or(0.0_f64, |worst| worst.rank) } else { f64::NEG_INFINITY };
            let rank = self.rank(&mut cursors, (docno, length), (essential, &bounds, threshold))?;
            let ranked = Ranked { rank, docno, doc };
            if self.after.is_some_and(|after| !ranked_after(&ranked, after)) {
                continue;
            }
            held.push(ranked);
            if held.len() > k {
                held.pop();
            }
            if held.len() >= k
                && let Some(worst) = held.peek()
            {
                while bounds.get(essential).is_some_and(|b| *b < worst.rank) {
                    essential = essential.saturating_add(1);
                }
            }
        }
        let mut best = held.into_vec();
        best.sort_unstable_by(|a, b| b.rank.total_cmp(&a.rank).then(a.docno.cmp(&b.docno)));
        Ok(best)
    }

    /// The rank of document `docno` of `length` words: the essential
    /// features at it, then the others while they may still reach the
    /// threshold.
    fn rank(
        &self,
        cursors: &mut [Cursor],
        (docno, length): (u64, u32),
        (essential, bounds, threshold): (usize, &[f64], f64),
    ) -> Result<f64> {
        let mut weights: Vec<(usize, f64)> = Vec::new();
        let mut rank = 0.0_f64;
        for (feature, cursor) in self.features.iter().zip(cursors.iter()).skip(essential) {
            if cursor.head() == Some(docno) {
                let weight = weighed(self.ranking, &self.corpus, feature.stats, (cursor.frequency(), length))?;
                weights.push((feature.order, weight));
                rank += weight;
            }
        }
        for at in (0..essential).rev() {
            if rank + bounds.get(at).copied().unwrap_or_default() < threshold {
                // It cannot reach the threshold: what it has, short of it.
                return Ok(rank);
            }
            let (Some(feature), Some(cursor)) = (self.features.get(at), cursors.get_mut(at)) else { break };
            if cursor.advance_to(docno)? == Some(docno) {
                let weight = weighed(self.ranking, &self.corpus, feature.stats, (cursor.frequency(), length))?;
                weights.push((feature.order, weight));
                rank += weight;
            }
        }
        weights.sort_unstable_by_key(|(order, _)| *order);
        Ok(weights.into_iter().map(|(_, weight)| weight).fold(0.0_f64, |sum, weight| sum + weight))
    }
}

/// Whether `ranked` comes after the document given last, `after`.
fn ranked_after(ranked: &Ranked, after: (f64, u64)) -> bool {
    match ranked.rank.total_cmp(&after.0) {
        Ordering::Less => true,
        Ordering::Greater => false,
        Ordering::Equal => ranked.docno > after.1,
    }
}

/// The least number at or past `docno` any of `cursors` holds.
fn least_head(cursors: &mut [Cursor], docno: u64) -> Result<Option<u64>> {
    let mut least: Option<u64> = None;
    for cursor in cursors {
        if let Some(found) = cursor.advance_to(docno)? {
            least = Some(least.map_or(found, |l| l.min(found)));
        }
    }
    Ok(least)
}

/// Where the stream is.
enum Phase {
    /// Ranking documents holding a term, in batches of this many.
    Ranking(usize),
    /// Giving every other document, of rank 0.
    Rest(Box<Rest>),
    /// Done.
    Done,
}

/// The documents of rank 0 still to give: every document, the documents
/// holding a term, and the least number not yet passed.
struct Rest {
    every: Every,
    holding: Option<Terms>,
    next: u64,
}

impl Rest {
    /// The rest of the documents of `index`, but those holding `terms`.
    fn new(index: &Arc<FullTextIndex>, terms: Vec<Vec<u8>>) -> Box<Rest> {
        let holding = (!terms.is_empty()).then(|| Terms::new(index, terms));
        Box::new(Rest { every: Every::new(index), holding, next: 0 })
    }

    /// The next document number holding no term.
    fn next(&mut self) -> Result<Option<u64>> {
        loop {
            let Some(docno) = self.every.advance_to(self.next)? else { return Ok(None) };
            self.next = docno.saturating_add(1);
            let held = match self.holding.as_mut() {
                Some(holding) => holding.advance_to(docno)? == Some(docno),
                None => false,
            };
            if !held {
                return Ok(Some(docno));
            }
        }
    }
}

/// The documents of an index best first for a pattern.
struct TopRanked {
    index: Arc<FullTextIndex>,
    features: Vec<Feature>,
    ranking: Arc<dyn Ranking>,
    corpus: Corpus,
    ready: VecDeque<Ranked>,
    after: Option<(f64, u64)>,
    phase: Phase,
    reservation: Reservation,
}

impl TopRanked {
    /// The next batch, or the phase after it.
    fn refill(&mut self) -> Result<()> {
        let Phase::Ranking(k) = self.phase else { return Ok(()) };
        self.reservation.resize(k.saturating_mul(HELD_BYTES))?;
        let pass = Pass { index: &self.index, features: &self.features, ranking: &*self.ranking, corpus: self.corpus, after: self.after };
        let best = pass.best(k)?;
        if best.len() < k {
            let terms = self.features.iter().map(|f| f.term.clone()).collect();
            self.phase = Phase::Rest(Rest::new(&self.index, terms));
        } else {
            self.phase = Phase::Ranking(k.saturating_mul(2).min(MOST_BATCH));
        }
        if let Some(last) = best.last() {
            self.after = Some((last.rank, last.docno));
        }
        self.ready.extend(best);
        Ok(())
    }

    /// The next document of rank 0, the stream done past the last.
    fn rest_once(&mut self) -> Result<Option<Ranked>> {
        let Phase::Rest(rest) = &mut self.phase else { return Ok(None) };
        while let Some(docno) = rest.next()? {
            if let Some((doc, _)) = self.index.numbered_with_length(docno)? {
                return Ok(Some(Ranked { rank: 0.0_f64, docno, doc }));
            }
        }
        self.phase = Phase::Done;
        Ok(None)
    }

    /// The next document, best first.
    fn take(&mut self) -> Result<Option<Ranked>> {
        loop {
            if let Some(ranked) = self.ready.pop_front() {
                return Ok(Some(ranked));
            }
            match self.phase {
                Phase::Ranking(_) => self.refill()?,
                Phase::Rest(..) => return self.rest_once(),
                Phase::Done => return Ok(None),
            }
        }
    }
}

impl Iterator for TopRanked {
    type Item = Result<(f64, Key, RowId)>;

    fn next(&mut self) -> Option<Result<(f64, Key, RowId)>> {
        match self.take() {
            Ok(Some(ranked)) => Some(Ok((ranked.rank, hash_key(&ranked.doc.hash), ranked.doc.row))),
            Ok(None) => None,
            Err(e) => {
                self.phase = Phase::Done;
                self.ready.clear();
                Some(Err(e))
            }
        }
    }
}

impl FullTextIndex {
    /// \[EXTENSION\] Every document of the index with its Rank for the
    /// pattern `pattern`, best first, of equal rank in number order
    /// [§5.1.3]: `(rank, key, row)`, the rank that `rank::score` gives
    /// the document by `statistics`, a snapshot the statement's Rank reads
    /// too, or one made now. A pattern's words that name no language are
    /// of each document's, so its words read in every language are
    /// counted: the rank is at least Rank's, and equal to it, bit for bit,
    /// where the pattern reads alike in each. None for a null pattern or
    /// one Contains refuses, which a scan answers, raising its error.
    ///
    /// # Errors
    ///
    /// The tree's and the statistics'; SQLCODE -918 if a batch cannot be
    /// reserved of `budget`; -910 if the ranking breaks its contract.
    pub fn ranked(
        self: Arc<Self>,
        pattern: &Value,
        statistics: Option<Arc<dyn Statistics>>,
        budget: &Arc<MemoryBudget>,
    ) -> Result<Option<NeighbourEntries>> {
        let Some(text) = crate::fulltext::types::pattern_of(pattern) else { return Ok(None) };
        let mut matchers: Vec<Matcher> = Vec::new();
        for language in self.languages().iter() {
            let Ok(read) = parse(text, language) else { return Ok(None) };
            if contains(&read, std::iter::empty(), language).is_err() {
                return Ok(None);
            }
            for matcher in positive_matchers(&read)? {
                if !matchers.contains(&matcher) {
                    matchers.push(matcher);
                }
            }
        }
        let statistics = match statistics {
            Some(statistics) => statistics,
            None => self.snapshot()?,
        };
        Ok(Some(Box::new(self.ranked_by(matchers, &*statistics, budget)?)))
    }

    /// As `ranked`, of the matchers of the pattern's words read in each
    /// language.
    fn ranked_by(self: Arc<Self>, matchers: Vec<Matcher>, statistics: &dyn Statistics, budget: &Arc<MemoryBudget>) -> Result<TopRanked> {
        let corpus = statistics.corpus();
        let ranking = self.ranking_of(statistics)?;
        let mut features = Vec::new();
        for matcher in matchers {
            let mut terms: Vec<(String, Vec<u8>)> =
                self.listed(&matcher)?.into_iter().map(|term| (String::from_utf8_lossy(&term).into_owned(), term)).collect();
            terms.sort_unstable();
            for (word, term) in terms {
                let stats = statistics.term(&word)?;
                if stats.documents == 0 {
                    continue;
                }
                let stats = TermStats { documents: stats.documents, most: stats.most.max(1) };
                let most = ranking.bound(&corpus, &stats);
                if !(most.is_finite() && most >= 0.0_f64) {
                    return Err(broken(&*ranking, "a bound negative or not finite"));
                }
                features.push(Feature { term, stats, most, order: features.len() });
            }
        }
        features.sort_by(|a, b| a.most.total_cmp(&b.most));
        let reservation = budget.reserve(FIRST_BATCH.saturating_mul(HELD_BYTES), "a full-text index's ranking")?;
        let phase = if features.is_empty() { Phase::Rest(Rest::new(&self, Vec::new())) } else { Phase::Ranking(FIRST_BATCH) };
        Ok(TopRanked { index: self, features, ranking, corpus, ready: VecDeque::new(), after: None, phase, reservation })
    }
}
