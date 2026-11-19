//! \[DESIGN\] The payloads of a ranking's calls (RULES.md D221), written
//! by the host and read by a program, and back:
//!
//! ```text
//! corpus   := documents u64 | average_length f64 | k1 f64 | b f64
//! term     := documents u64 | most u32
//! WEIGHTS  corpus | count u32 | (term | frequency u32 | length u32)...   -> count u32 | f64...
//! BOUNDS   corpus | count u32 | term...                                  -> count u32 | f64...
//! ```
//!
//! A float crosses as its bits, a u64.

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::fulltext::rank::{Corpus, InDocument, TermStats};

use super::{count, put_f64, read_f64};

/// `corpus`, written.
pub fn put_corpus(out: &mut Vec<u8>, corpus: &Corpus) {
    out.put_u64(corpus.documents);
    for x in [corpus.average_length, corpus.k1, corpus.b] {
        put_f64(out, x);
    }
}

/// A corpus, read.
///
/// # Errors
///
/// `Reader`'s, if it holds none.
pub fn corpus(r: &mut Reader<'_>) -> Result<Corpus> {
    Ok(Corpus { documents: r.u64()?, average_length: read_f64(r)?, k1: read_f64(r)?, b: read_f64(r)? })
}

/// `term`, written.
pub fn put_term(out: &mut Vec<u8>, term: &TermStats) {
    out.put_u64(term.documents);
    out.put_u32(term.most);
}

/// A term, read.
///
/// # Errors
///
/// `Reader`'s.
pub fn term(r: &mut Reader<'_>) -> Result<TermStats> {
    Ok(TermStats { documents: r.u64()?, most: r.u32()? })
}

/// WEIGHTS's payload: the corpus and each term in its document.
pub fn put_weights(out: &mut Vec<u8>, corpus: &Corpus, asked: &[(TermStats, InDocument)]) {
    put_corpus(out, corpus);
    out.put_u32(count(asked.len()));
    for (t, d) in asked {
        put_term(out, t);
        out.put_u32(d.frequency);
        out.put_u32(d.length);
    }
}

/// What `put_weights` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn weights(r: &mut Reader<'_>) -> Result<(Corpus, Vec<(TermStats, InDocument)>)> {
    let c = corpus(r)?;
    let asked = (0..r.u32()?).map(|_| Ok((term(r)?, InDocument { frequency: r.u32()?, length: r.u32()? }))).collect::<Result<_>>()?;
    Ok((c, asked))
}

/// BOUNDS's payload: the corpus and each term.
pub fn put_bounds(out: &mut Vec<u8>, corpus: &Corpus, terms: &[TermStats]) {
    put_corpus(out, corpus);
    out.put_u32(count(terms.len()));
    for t in terms {
        put_term(out, t);
    }
}

/// What `put_bounds` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn bounds(r: &mut Reader<'_>) -> Result<(Corpus, Vec<TermStats>)> {
    let c = corpus(r)?;
    let terms = (0..r.u32()?).map(|_| term(r)).collect::<Result<_>>()?;
    Ok((c, terms))
}
