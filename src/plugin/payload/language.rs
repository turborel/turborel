//! \[DESIGN\] The payloads of a language's calls (RULES.md D328), each a
//! batch: many documents cut into words a call, many words stemmed a
//! call, and the stop words asked once, as the extension is registered.
//!
//! ```text
//! span     := offset u32 | length u32, in characters
//! SEGMENT  count u32 | str...   -> count u32 | (count u32 | span...)...
//! STEM     count u32 | str...   -> count u32 | str...
//! STOP     (nothing)            -> count u32 | str...
//! ```

use crate::bytes::{Put, Reader};
use crate::error::Result;

use super::count;

/// The documents or words of SEGMENT or STEM.
pub fn put_texts(out: &mut Vec<u8>, texts: &[&str]) {
    out.put_u32(count(texts.len()));
    for t in texts {
        out.put_str(t);
    }
}

/// SEGMENT's answer: the words of each document.
pub fn put_spans(out: &mut Vec<u8>, all: &[Vec<(u32, u32)>]) {
    out.put_u32(count(all.len()));
    for spans in all {
        out.put_u32(count(spans.len()));
        for (offset, length) in spans {
            out.put_u32(*offset);
            out.put_u32(*length);
        }
    }
}

/// What `put_spans` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn spans(r: &mut Reader<'_>) -> Result<Vec<Vec<(u32, u32)>>> {
    (0..r.u32()?).map(|_| (0..r.u32()?).map(|_| Ok((r.u32()?, r.u32()?))).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::payload::{reader, strings};

    /// Verifies: [EXTENSION] RULES.md D328: a language's payloads read
    /// back what was written, documents and words as strings and each
    /// document's words as its spans.
    #[test]
    fn spans_and_texts_round_trip() {
        let mut texts = Vec::new();
        put_texts(&mut texts, &["数据库", "a b"]);
        assert_eq!(strings(&mut reader(&texts)).unwrap(), ["数据库", "a b"]);
        let all = vec![vec![(0, 3)], vec![(0, 1), (2, 1)], Vec::new()];
        let mut out = Vec::new();
        put_spans(&mut out, &all);
        let mut r = reader(&out);
        assert_eq!(spans(&mut r).unwrap(), all);
        assert!(r.finish().is_ok());
    }
}
