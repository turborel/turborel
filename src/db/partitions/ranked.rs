//! \[EXTENSION\] The ranking statistics of a full-text index of a
//! partitioned table (RULES.md D158): its partitions' indexes' together,
//! as one index of all their rows would hold them. The documents are the
//! sum of theirs, their average length weighed by each one's documents,
//! and a term's documents the sum of each one's, its most in a document
//! the most of any. So Rank scores a row of a partition as it would score
//! it in a table not partitioned, whichever partitions a read prunes, and
//! the bounds a top-k read of one partition's index takes agree with the
//! scores its rows are given.

use std::sync::Arc;

use crate::db::State;
use crate::error::Result;
use crate::fulltext::Language;
use crate::fulltext::rank::{Corpus, Statistics, TermStats};
use crate::fulltext::text::StopWords;

/// The statistics of several indexes together, the first's options in
/// force: each index's snapshot, taken once.
struct Together {
    each: Vec<Arc<dyn Statistics>>,
    corpus: Corpus,
}

impl State {
    /// The ranking statistics of the template `name`, of a partitioned
    /// table: its indexes' together; none if it is no template or none of
    /// its indexes ranks.
    ///
    /// # Errors
    ///
    /// As `Index::ranking_statistics`.
    pub(crate) fn template_statistics(&self, name: &str) -> Result<Option<Arc<dyn Statistics>>> {
        let Some((table, _)) = self.template(name) else { return Ok(None) };
        let mut each = Vec::new();
        for made in self.made_of_template(&table, name) {
            let Ok(ix) = self.index_of(&made) else { continue };
            if let Some(stats) = Arc::clone(ix).ranking_statistics()? {
                each.push(stats);
            }
        }
        let Some(first) = each.first() else { return Ok(None) };
        let mut corpus = first.corpus();
        let (mut documents, mut words) = (0_u64, 0.0_f64);
        for s in &each {
            let c = s.corpus();
            documents = documents.saturating_add(c.documents);
            words += c.average_length * counted(c.documents);
        }
        corpus.documents = documents;
        corpus.average_length = if documents == 0 { 0.0_f64 } else { words / counted(documents) };
        Ok(Some(Arc::new(Together { each, corpus })))
    }
}

/// `n` as a float, near enough for an average.
fn counted(n: u64) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

impl Statistics for Together {
    fn corpus(&self) -> Corpus {
        self.corpus
    }

    fn ranking(&self) -> &str {
        self.each.first().map_or(crate::fulltext::rank::BM25, |s| s.ranking())
    }

    fn term(&self, term: &str) -> Result<TermStats> {
        let mut held = TermStats { documents: 0, most: 0 };
        for s in &self.each {
            let t = s.term(term)?;
            held.documents = held.documents.saturating_add(t.documents);
            held.most = held.most.max(t.most);
        }
        held.documents = held.documents.min(self.corpus.documents);
        Ok(held)
    }

    fn stop_words(&self) -> StopWords {
        self.each.first().map_or(StopWords::Language, |s| s.stop_words())
    }

    fn most_term(&self) -> usize {
        self.each.first().map_or(crate::index::fulltext::MOST_TERM_BYTES, |s| s.most_term())
    }

    fn language(&self) -> Option<Language> {
        self.each.first().and_then(|s| s.language())
    }
}
