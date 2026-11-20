//! \[DESIGN\] Rank [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.1.3, §4.3]:
//! how relevant a document is to a pattern, by a ranking over a snapshot
//! of statistics (`crate::fulltext::rank`): the sum over the words the
//! pattern asks for that are not under NOT, of each term a word matches in
//! the document (`matcher.rs`), of the ranking's weight of the term there.
//!
//! A document's terms are counted as its index posts them: its words not
//! stop words, each kept under its term, words alike in the term's bytes
//! counted as one. The weights are summed in one order, by the pattern's
//! words then their terms' bytes, so Rank and an index's top-k (`topk.rs`)
//! give one document the same value, bit for bit. The value is never
//! negative and at most the sum of the ranking's bounds of the pattern's
//! terms, as the standard's minimum and maximum ask [§5.1.3 Description
//! 3b ii].
//!
//! A snapshot (`Snapshot`) reads the index's totals once, when it is made,
//! and each term's statistics once, when first asked, remembering them for
//! the statement; what it remembers is held against the memory budget, as
//! a mask may match many terms.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use super::FullTextIndex;
use super::document::term_of;
use super::matcher::positive_matchers;
use crate::error::{Code, Result, SqlError};
use crate::fulltext::pattern::Pattern;
use crate::fulltext::rank::{Corpus, InDocument, Ranking, Statistics, TermStats};
use crate::fulltext::text::{StopWords, tokenize_with};
use crate::fulltext::value::{self, Document};
use crate::fulltext::{Language, Languages};
use crate::memory::Reservation;
use crate::sync::lock_builtin;
use crate::value::Value;

/// \[DESIGN\] How far a weight may pass its bound before the ranking is
/// taken to break its contract: floating point's error.
pub(super) const BOUND_SLACK: f64 = 1e-9;

/// \[DESIGN\] The bytes a remembered term is reserved for, past its own.
const TERM_BYTES: usize = 64;

/// The frequency in a document of each term each of a pattern's positive
/// words matches, by word then term, and the document's length.
type Counted = (Vec<BTreeMap<String, u32>>, u32);

/// The counts of `document`, read in its language or else `language`.
fn counted(pattern: &Pattern, document: Document<'_>, language: Language, statistics: &dyn Statistics) -> Result<Counted> {
    let matchers = positive_matchers(pattern)?;
    let language = document.language.unwrap_or(language);
    let stop = statistics.stop_words();
    let most = statistics.most_term();
    let mut counts: Vec<BTreeMap<String, u32>> = vec![BTreeMap::new(); matchers.len()];
    let mut length = 0_u32;
    for token in tokenize_with(document.chars, language, &stop) {
        let token = token?;
        if token.stop {
            continue;
        }
        length = length.saturating_add(1);
        for (matcher, count) in matchers.iter().zip(counts.iter_mut()) {
            if matcher.matches(&token.word, Languages::of(language))? {
                let term = String::from_utf8_lossy(term_of(&token.word, most)).into_owned();
                let tf = count.entry(term).or_default();
                *tf = tf.saturating_add(1);
            }
        }
    }
    Ok((counts, length))
}

/// The error of a ranking that broke its contract (RULES.md R-SAFE-3).
pub(super) fn broken(ranking: &dyn Ranking, what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[EXTENSION] the ranking {} broke its contract: {what}", ranking.name()))
}

/// `ranking`'s weight of `term`, of frequency `frequency` in a document of
/// `length` words, checked against its bound: a term a document holds more
/// often than the snapshot says, as one indexed since, of that frequency
/// at most.
///
/// # Errors
///
/// SQLCODE -910 if it is past the bound, negative or not finite.
pub(super) fn weighed(ranking: &dyn Ranking, corpus: &Corpus, term: TermStats, (frequency, length): (u32, u32)) -> Result<f64> {
    let (term, in_doc) = asked(term, (frequency, length));
    checked(ranking, corpus, &term, ranking.weight(corpus, &term, &in_doc))
}

/// The term `term` of a document, of `frequency` there in a document of
/// `length` words, as `weighed` weighs it.
fn asked(term: TermStats, (frequency, length): (u32, u32)) -> (TermStats, InDocument) {
    let term = TermStats { documents: term.documents.max(1), most: term.most.max(frequency) };
    (term, InDocument { frequency, length: length.max(frequency) })
}

/// `weight`, of `term`, if within its bound, finite and not negative.
fn checked(ranking: &dyn Ranking, corpus: &Corpus, term: &TermStats, weight: f64) -> Result<f64> {
    let bound = ranking.bound(corpus, term);
    if weight.is_finite() && weight >= 0.0_f64 && weight <= bound + BOUND_SLACK {
        Ok(weight)
    } else {
        Err(broken(ranking, "a weight past its bound, negative or not finite"))
    }
}

/// \[EXTENSION\] The rank of `document` for `pattern` by `ranking` over
/// `statistics` [§5.1.3]; a character string read in `language`. The
/// caller gives a null document's or pattern's Rank as null.
///
/// # Errors
///
/// As reading the document and the statistics; SQLCODE -910 if the
/// ranking gives a weight that is not finite and non-negative, or past its
/// bound.
pub(crate) fn score(
    pattern: &Pattern,
    document: Document<'_>,
    language: Language,
    statistics: &dyn Statistics,
    ranking: &dyn Ranking,
) -> Result<f64> {
    let (counts, length) = counted(pattern, document, language, statistics)?;
    let corpus = statistics.corpus();
    let mut all = Vec::new();
    for count in counts {
        for (term, frequency) in count {
            all.push(asked(statistics.term(&term)?, (frequency, length)));
        }
    }
    // One question of the ranking for every term of the document, as a
    // plug-in's is a round trip (RULES.md D221).
    let weights = ranking.weights(&corpus, &all);
    if weights.len() != all.len() {
        return Err(broken(ranking, "a weight for each term was not given"));
    }
    let mut total = 0.0_f64;
    for ((term, _), weight) in all.iter().zip(weights) {
        total += checked(ranking, &corpus, term, weight)?;
    }
    Ok(total)
}

/// \[DESIGN\] An index's statistics for one statement: its totals and
/// weights as they were when it was made, each term's as it was when first
/// read.
pub(super) struct Snapshot {
    /// The index.
    index: Arc<FullTextIndex>,
    /// Its collection and weights.
    corpus: Corpus,
    /// Its ranking's name.
    ranking: String,
    /// The terms read, and the memory they are held in.
    terms: Mutex<(HashMap<String, TermStats>, Reservation)>,
}

impl Statistics for Snapshot {
    fn corpus(&self) -> Corpus {
        self.corpus
    }
    fn ranking(&self) -> &str {
        &self.ranking
    }
    fn term(&self, term: &str) -> Result<TermStats> {
        let mut held = lock_builtin(&self.terms);
        let (terms, reservation) = &mut *held;
        if let Some(found) = terms.get(term) {
            return Ok(*found);
        }
        let read = self.index.statistics(term)?;
        let found = TermStats { documents: read.documents, most: read.most };
        let more = reservation.bytes().saturating_add(term.len()).saturating_add(TERM_BYTES);
        reservation.resize(more)?;
        terms.insert(term.to_owned(), found);
        Ok(found)
    }
    fn stop_words(&self) -> StopWords {
        self.index.reading.stop.clone()
    }
    fn most_term(&self) -> usize {
        self.index.most_term
    }
    fn language(&self) -> Option<Language> {
        Some(self.index.reading.language)
    }
}

impl FullTextIndex {
    /// \[EXTENSION\] A snapshot of its statistics for one statement: its
    /// totals and options read now, each term's when first asked.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if the totals are malformed; -1 if the
    /// memory budget is spent.
    pub fn snapshot(self: &Arc<Self>) -> Result<Arc<dyn Statistics>> {
        let totals = self.totals()?;
        let weights = self.weights();
        let average_length = if totals.documents == 0 { 0.0_f64 } else { count(totals.length) / count(totals.documents) };
        let corpus = Corpus { documents: totals.documents, average_length, k1: weights.k1, b: weights.b };
        let reservation = self.spill.budget.reserve(0, "a full-text index's statistics")?;
        let ranking = self.ranking().name().to_owned();
        Ok(Arc::new(Snapshot { index: Arc::clone(self), corpus, ranking, terms: Mutex::new((HashMap::new(), reservation)) }))
    }

    /// \[EXTENSION\] The rank of `document`, a FullText value or a
    /// character string, for `pattern` by `statistics` and the ranking
    /// they name [§5.1.3], as `score` gives it; none of a null document.
    ///
    /// # Errors
    ///
    /// As `score`; SQLCODE -900 if `document` is no text.
    pub fn rank(&self, pattern: &Pattern, document: &Value, statistics: &dyn Statistics) -> Result<Option<f64>> {
        let Some(document) = value::document(document)? else { return Ok(None) };
        let ranking = self.ranking_of(statistics)?;
        score(pattern, document, self.reading.language, statistics, &*ranking).map(Some)
    }

    /// The ranking `statistics` names, among those this index may name.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is none of that name, as one unregistered
    /// since.
    pub(super) fn ranking_of(&self, statistics: &dyn Statistics) -> Result<Arc<dyn Ranking>> {
        crate::fulltext::rank::named(&self.rankings, statistics.ranking())
            .ok_or_else(|| SqlError::new(Code::Api, format!("[EXTENSION] no ranking {}", statistics.ranking())))
    }
}

/// A count as a float, rounded past 2^53.
fn count(value: u64) -> f64 {
    let high = u32::try_from(value >> 32_u32).unwrap_or(u32::MAX);
    let low = u32::try_from(value & u64::from(u32::MAX)).unwrap_or(u32::MAX);
    f64::from(high) * 4_294_967_296.0_f64 + f64::from(low)
}
