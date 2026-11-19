//! \[EXTENSION\] Rankings: how Rank weighs a document for a pattern
//! [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.1.3, §4.3], a plug-in's
//! (`Database::register_ranking`) or the built-in BM25 (RULES.md D86). A
//! pattern's rank is the sum, over the terms its words not under NOT match
//! in the document, of the ranking's weight of each
//! (`index::fulltext::score`). An index names its ranking by its option
//! FULLTEXT.RANKING, which changes in place, as postings do not depend on
//! it.
//!
//! Rank is implementation-dependent, "constrained by implementation-
//! defined minimum and maximum values" [§5.1.3 Description 3b ii]: here at
//! least 0, and at most the sum over the pattern's terms of the ranking's
//! bound, finite.

use std::sync::Arc;

use crate::error::Result;
use crate::fulltext::Language;
use crate::fulltext::text::StopWords;
use crate::index::fulltext::MOST_TERM_BYTES;

/// The name of the built-in ranking, BM25.
pub const BM25: &str = "BM25";

/// The collection a document is ranked in, and the index's options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corpus {
    /// The documents indexed.
    pub documents: u64,
    /// Their average length, in words not stop words; 0 with none.
    pub average_length: f64,
    /// FULLTEXT.K1: how fast a term's frequency saturates.
    pub k1: f64,
    /// FULLTEXT.B: how much a document's length counts, from 0 to 1.
    pub b: f64,
}

/// A term of the collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermStats {
    /// The documents holding it: its document frequency, at most the
    /// corpus's documents.
    pub documents: u64,
    /// At least the most times it occurs in one document.
    pub most: u32,
}

/// A term in one document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InDocument {
    /// Its frequency there, at least 1 and at most the term's `most`.
    pub frequency: u32,
    /// The document's length, in words not stop words, at least the
    /// frequency.
    pub length: u32,
}

/// \[EXTENSION\] A ranking: a term's weight in a document, from the
/// collection's statistics (RULES.md R-SAFE-3).
///
/// Its contract, which every implementation, built in or a plug-in's,
/// **shall** keep:
///
/// * `name` is fixed, an upper-case identifier, unique among the
///   database's rankings.
/// * `weight` and `bound` are deterministic: the same arguments give the
///   same value, bit for bit.
/// * Each gives a finite value, never negative, and never panics.
/// * `weight(c, t, d) <= bound(c, t)` for every `d` the contract of
///   `InDocument` allows: a full-text index finds the best documents first
///   by these bounds, and refuses a ranking it finds giving more (SQLCODE
///   -910), so a top-k is never wrong.
/// * Neither calls back into the `Database` or its sessions (R-ERR-4), nor
///   holds a lock past its return.
pub trait Ranking: Send + Sync {
    /// Its name, as FULLTEXT.RANKING names it.
    fn name(&self) -> &str;
    /// The weight of `term` in a document where it is `in_doc`.
    fn weight(&self, corpus: &Corpus, term: &TermStats, in_doc: &InDocument) -> f64;
    /// The most `weight` gives `term` in any document of `corpus`.
    fn bound(&self, corpus: &Corpus, term: &TermStats) -> f64;
    /// \[DESIGN\] The `weight` of each term of `asked` in its document,
    /// together: one round trip of a plug-in's ranking for a document's
    /// terms (RULES.md D221). Default: `weight` of each.
    fn weights(&self, corpus: &Corpus, asked: &[(TermStats, InDocument)]) -> Vec<f64> {
        asked.iter().map(|(t, d)| self.weight(corpus, t, d)).collect()
    }
}

/// \[EXTENSION\] A collection's statistics as Rank reads them in one
/// statement: a snapshot, whose `corpus` is fixed when it is made and whose
/// `term` gives one term the same statistics each time it is asked, so a
/// document's rank, and the bounds of a top-k, agree however often they
/// are read (RULES.md D86).
pub trait Statistics: Send + Sync {
    /// The documents, their average length, and the weights in force.
    fn corpus(&self) -> Corpus;
    /// The name of the ranking in force (FULLTEXT.RANKING).
    fn ranking(&self) -> &str;
    /// The statistics of `term`, a word normalized and cut to
    /// `most_term` bytes; of no document if none holds it.
    ///
    /// # Errors
    ///
    /// As reading them; SQLCODE -1 (no memory) if the terms remembered
    /// pass the memory budget.
    fn term(&self, term: &str) -> Result<TermStats>;
    /// The words not counted. Default: each document's language's.
    fn stop_words(&self) -> StopWords {
        StopWords::Language
    }
    /// The bytes a term is cut to. Default: an index's.
    fn most_term(&self) -> usize {
        MOST_TERM_BYTES
    }
    /// The language a character string is read in, an index's
    /// FULLTEXT.LANGUAGE; none of no index. Default: none.
    fn language(&self) -> Option<Language> {
        None
    }
}

/// \[EXTENSION\] The statistics of no collection, for Rank of a column no
/// index ranks: BM25's, the built-in, with every inverse document
/// frequency 1 and every document of average length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoIndex {
    /// FULLTEXT.K1's default.
    pub k1: f64,
    /// FULLTEXT.B's default.
    pub b: f64,
}

impl Statistics for NoIndex {
    fn corpus(&self) -> Corpus {
        Corpus { documents: 0, average_length: 0.0_f64, k1: self.k1, b: self.b }
    }
    fn ranking(&self) -> &str {
        BM25
    }
    fn term(&self, _term: &str) -> Result<TermStats> {
        Ok(TermStats { documents: 0, most: 0 })
    }
}

/// The rankings an index may name, as they are registered now: the
/// database's registry, read at each CREATE and ALTER INDEX, or the
/// built-in alone.
pub type Rankings = Arc<dyn Fn() -> Vec<Arc<dyn Ranking>> + Send + Sync>;

/// The built-in rankings alone: BM25.
pub fn built_in() -> Rankings {
    Arc::new(|| {
        let bm25: Arc<dyn Ranking> = Arc::new(Bm25);
        vec![bm25]
    })
}

/// The ranking of `rankings` named `name`, its case not counting.
pub fn named(rankings: &Rankings, name: &str) -> Option<Arc<dyn Ranking>> {
    rankings().into_iter().find(|ranking| ranking.name().eq_ignore_ascii_case(name))
}

/// \[EXTENSION\] Okapi BM25 (Robertson and Walker, SIGIR 1994; Robertson et
/// al., TREC-3): of a term of frequency `tf` in a document of length
/// `dl`,
///
/// `idf · tf · (k1 + 1) / (tf + k1 · (1 − b + b · dl / avgdl))`
///
/// with `idf = ln(1 + (N − df + 0.5) / (df + 0.5))`, never negative; 1 of
/// a corpus of no documents, as of no index (`NoIndex`). Its
/// bound is the weight at the term's greatest frequency in a document of
/// length 0: the weight grows with `tf`, as its derivative in `tf` is
/// `idf · (k1 + 1) · K / (tf + K)²` with `K = k1 · (1 − b + b · dl /
/// avgdl) >= 0`, and falls as `dl` grows, as `K` does not fall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bm25;

impl Bm25 {
    /// The inverse document frequency of a term `df` of `n` documents hold;
    /// 1 if `n` is 0.
    pub fn idf(n: u64, df: u64) -> f64 {
        if n == 0 {
            return 1.0_f64;
        }
        let (n, df) = (count(n), count(df.min(n)));
        (1.0_f64 + (n - df + 0.5_f64) / (df + 0.5_f64)).ln()
    }

    /// The weight of a term of frequency `tf` and inverse document
    /// frequency `idf` in a document of length `ratio` times the average,
    /// by k1 and b of `corpus`.
    pub fn of(corpus: &Corpus, idf: f64, tf: u32, ratio: f64) -> f64 {
        if tf == 0 {
            return 0.0_f64;
        }
        let k1 = finite(corpus.k1);
        let b = finite(corpus.b).clamp(0.0_f64, 1.0_f64);
        let tf = f64::from(tf);
        let norm = k1 * (1.0_f64 - b + b * finite(ratio).max(0.0_f64));
        let weight = idf * tf * (k1 + 1.0_f64) / (tf + norm);
        if weight.is_finite() { weight.max(0.0_f64) } else { 0.0_f64 }
    }
}

/// A document of `length` words over the average `average`; 1 with no
/// average.
pub fn ratio(length: u32, average: f64) -> f64 {
    if average > 0.0_f64 { f64::from(length) / average } else { 1.0_f64 }
}

/// `value` if finite, else 0.
fn finite(value: f64) -> f64 {
    if value.is_finite() { value.max(0.0_f64) } else { 0.0_f64 }
}

/// A count as a float, rounded past 2^53.
fn count(value: u64) -> f64 {
    let high = u32::try_from(value >> 32_u32).unwrap_or(u32::MAX);
    let low = u32::try_from(value & u64::from(u32::MAX)).unwrap_or(u32::MAX);
    f64::from(high) * 4_294_967_296.0_f64 + f64::from(low)
}

impl Ranking for Bm25 {
    fn name(&self) -> &str {
        BM25
    }
    fn weight(&self, corpus: &Corpus, term: &TermStats, in_doc: &InDocument) -> f64 {
        let idf = Bm25::idf(corpus.documents, term.documents);
        Bm25::of(corpus, idf, in_doc.frequency, ratio(in_doc.length, corpus.average_length))
    }
    fn bound(&self, corpus: &Corpus, term: &TermStats) -> f64 {
        Bm25::of(corpus, Bm25::idf(corpus.documents, term.documents), term.most, 0.0_f64)
    }
}
