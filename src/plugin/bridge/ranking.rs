//! \[EXTENSION\] A plug-in's ranking of full-text search (RULES.md D221),
//! a `Ranking` whose weights of a document's terms are one CALL
//! (`weights`), and whose answers are kept, as a ranking is deterministic
//! by its contract, so a weight or bound asked again makes no call.
//!
//! `Ranking` gives no error, so a call that fails gives a weight or bound
//! that is not a number, which the index refuses as a broken contract
//! (SQLCODE -910): the statement fails, and no rank is ever wrong.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::fulltext::rank::{Corpus, InDocument, Ranking, TermStats};
use crate::plugin::payload::ranking::{put_bounds, put_weights};
use crate::plugin::payload::{RANKING_BOUNDS, RANKING_WEIGHTS, f64s};
use crate::sync::lock_builtin;

use super::{Link, answer, counted};

/// \[DESIGN\] The answers kept at most, of weights and of bounds each:
/// enough for the terms and document lengths of many statements, little
/// memory (about 40 bytes each); past it they are forgotten together.
pub const KEPT: usize = 65_536;

/// What a weight is asked by: the corpus's bits, the term, the document.
type WeightKey = ([u64; 4], (u64, u32), (u32, u32));
/// What a bound is asked by.
type BoundKey = ([u64; 4], (u64, u32));

/// A plug-in's ranking.
#[derive(Debug)]
pub struct PluginRanking {
    link: Link,
    weights: Mutex<HashMap<WeightKey, f64>>,
    bounds: Mutex<HashMap<BoundKey, f64>>,
}

/// The bits of `c`, as answers are kept by.
fn bits(c: &Corpus) -> [u64; 4] {
    [c.documents, c.average_length.to_bits(), c.k1.to_bits(), c.b.to_bits()]
}

/// `value` kept in `map` under `key`, the map emptied first if full.
fn keep<K: std::hash::Hash + Eq>(map: &Mutex<HashMap<K, f64>>, key: K, value: f64) {
    let mut held = lock_builtin(map);
    if held.len() >= KEPT {
        held.clear();
    }
    held.insert(key, value);
}

impl PluginRanking {
    /// The ranking `link` names.
    pub fn new(link: Link) -> PluginRanking {
        PluginRanking { link, weights: Mutex::new(HashMap::new()), bounds: Mutex::new(HashMap::new()) }
    }

    /// The weights of `asked` the program gives, one CALL; none if it
    /// fails.
    fn asked(&self, corpus: &Corpus, asked: &[(TermStats, InDocument)]) -> Option<Vec<f64>> {
        let mut payload = Vec::new();
        put_weights(&mut payload, corpus, asked);
        let got = self.link.plain(RANKING_WEIGHTS, payload).ok()?;
        answer(&got, f64s).and_then(|w| counted(w, asked.len())).ok()
    }
}

impl Ranking for PluginRanking {
    fn name(&self) -> &str {
        &self.link.object
    }
    fn weight(&self, corpus: &Corpus, term: &TermStats, in_doc: &InDocument) -> f64 {
        self.weights(corpus, &[(*term, *in_doc)]).pop().unwrap_or(f64::NAN)
    }
    fn bound(&self, corpus: &Corpus, term: &TermStats) -> f64 {
        let key = (bits(corpus), (term.documents, term.most));
        if let Some(found) = lock_builtin(&self.bounds).get(&key) {
            return *found;
        }
        let mut payload = Vec::new();
        put_bounds(&mut payload, corpus, &[*term]);
        let got = self.link.plain(RANKING_BOUNDS, payload).ok().and_then(|got| answer(&got, f64s).ok());
        let Some(bound) = got.and_then(|mut b| b.pop()) else { return f64::NAN };
        keep(&self.bounds, key, bound);
        bound
    }
    fn weights(&self, corpus: &Corpus, asked: &[(TermStats, InDocument)]) -> Vec<f64> {
        let key = |(t, d): &(TermStats, InDocument)| (bits(corpus), (t.documents, t.most), (d.frequency, d.length));
        let mut found: Vec<Option<f64>> = {
            let held = lock_builtin(&self.weights);
            asked.iter().map(|a| held.get(&key(a)).copied()).collect()
        };
        let missing: Vec<(TermStats, InDocument)> = asked.iter().zip(&found).filter(|(_, f)| f.is_none()).map(|(a, _)| *a).collect();
        if !missing.is_empty() {
            let got = self.asked(corpus, &missing).unwrap_or_else(|| vec![f64::NAN; missing.len()]);
            let mut got = missing.iter().zip(got);
            for slot in found.iter_mut().filter(|f| f.is_none()) {
                let Some((a, w)) = got.next() else { break };
                if !w.is_nan() {
                    keep(&self.weights, key(a), w);
                }
                *slot = Some(w);
            }
        }
        found.into_iter().map(|f| f.unwrap_or(f64::NAN)).collect()
    }
}
