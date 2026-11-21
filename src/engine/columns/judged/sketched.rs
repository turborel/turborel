//! \[DESIGN\] (RULES.md D525) A conjunct of a column of texts coded by
//! symbols judged by its group's sketch (`sketch.rs`) first. Of each
//! string constant of the conjunct, a binary search finds its place among
//! the sketch's edges. Against it, every value of a code orders alike,
//! told by places alone, but the values of the code whose bin holds the
//! constant between its bounds. So the conjunct is judged once a code,
//! as `text_keeps` judges a text, each constant's order the code's. Of a
//! code it is true of every value of, a row is kept by its code alone;
//! of one it is false of every value of, the row is left; and of a code
//! a constant orders variously against, a boundary, the row's text is
//! compared as before. A null's code is judged as the conjunct is of a
//! null.

use std::cell::Cell;
use std::cmp::Ordering;

use super::super::sketch::{Edges, NULL_CODE, Verdict, Verdicts};
use super::{Pred, sorted, text_keeps};

/// \[DESIGN\] The most constants found by their address alone, past
/// which they are found by a binary search of their texts.
const FEW: usize = 16;

/// What each code of `edges` says of `pred`: none if the conjunct is not
/// judged of text by constants (`text_keeps`, `sorted::constants_of`).
pub(super) fn verdicts(edges: &Edges, pred: &Pred) -> Option<Verdicts> {
    let mut constants = Vec::new();
    sorted::constants_of(&pred.conjunct, &mut constants)?;
    let mut placed: Vec<(&str, (usize, bool))> = constants.iter().map(|&c| (c, edges.place(c.as_bytes()))).collect();
    placed.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let find = |c: &str| {
        if placed.len() <= FEW {
            return placed.iter().find(|p| std::ptr::eq(p.0, c)).or_else(|| placed.iter().find(|p| p.0 == c)).map(|p| p.1);
        }
        placed.binary_search_by(|p| p.0.cmp(c)).ok().and_then(|at| placed.get(at)).map(|p| p.1)
    };
    let mut out = Verdicts::checked();
    out.set(NULL_CODE, if pred.of_null { Verdict::Keep } else { Verdict::Drop });
    for (code, bin) in edges.bins() {
        let mixed = Cell::new(false);
        let order = |c: &str| {
            let o = find(c).and_then(|place| bin.order(place));
            mixed.set(mixed.get() || o.is_none());
            Some(o.unwrap_or(Ordering::Equal))
        };
        let keeps = text_keeps(&pred.conjunct, &order)?;
        if !mixed.get() {
            out.set(code, if keeps { Verdict::Keep } else { Verdict::Drop });
        }
    }
    Some(out)
}

// Unit tests: what each code says, of every kind of conjunct, against the
// texts it holds (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
