//! \[EXTENSION\] A string split into collation units, as XQuery's substring
//! matching asks [XQueryFO §7.5] (RULES.md D60, D106): each unit a
//! character the collation's steps begin at, a starter its normalized form
//! begins with, and what follows it up to the next such character, so a
//! contraction, or a character and its marks, is one unit; weighed at each
//! level as the collation weighs the whole string, each element's weights
//! the unit's that holds it, so the units' weights, joined, are the
//! string's.

/// A collation unit [XQueryFO §7.5]: where it begins, and its weights.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// The place among the string's characters of the first it holds.
    pub start: usize,
    /// Its weights at each level the collation compares, as
    /// `Collation::level_weights` gives a string's.
    pub weights: Vec<Vec<u8>>,
}

/// The elements a collation's steps make of the normalized characters
/// `chars`, and where each unit begins: the place of its first element,
/// and of its first character in the string. `begins` gives, for each of
/// `chars`, the string's character it begins if it begins one with a
/// starter; a unit begins at each such character a step begins at, and
/// the first unit at the string's start. A step makes the elements of the
/// characters at `at` and returns where the next begins; it may take
/// marks out of `chars` after that (S2.1.1 of \[UTS #10\]), each of which
/// begins no character, so as many entries of none are taken out of
/// `begins` there.
pub(super) fn split<E>(
    mut chars: Vec<char>,
    mut begins: Vec<Option<usize>>,
    mut step: impl FnMut(&mut Vec<char>, usize, &mut Vec<E>) -> usize,
) -> (Vec<E>, Vec<(usize, usize)>) {
    let (mut elements, mut starts) = (Vec::with_capacity(chars.len()), Vec::new());
    let mut at = 0;
    while at < chars.len() {
        match begins.get(at).copied().flatten() {
            Some(start) => starts.push((elements.len(), start)),
            None => {
                if starts.is_empty() {
                    starts.push((elements.len(), 0));
                }
            }
        }
        let held = chars.len();
        let next = step(&mut chars, at, &mut elements);
        // What follows `next` up to the marks taken is marks, so its entries
        // are all none, and taking the first as many is taking theirs.
        for _ in 0..held.saturating_sub(chars.len()) {
            if begins.get(next).is_some_and(Option::is_none) {
                begins.remove(next);
            }
        }
        at = next;
    }
    (elements, starts)
}

/// The units whose elements begin as `starts` gives, each element's
/// weights at each of `levels` levels in `per`, a weight of the default
/// adding nothing, each written by `put`.
pub(super) fn weighed<W: Copy + Default + PartialEq, const N: usize>(
    starts: &[(usize, usize)],
    per: &[[W; N]],
    levels: usize,
    put: impl Fn(W, &mut Vec<u8>),
) -> Vec<Unit> {
    let mut units = Vec::with_capacity(starts.len());
    for (n, &(first, start)) in starts.iter().enumerate() {
        let end = starts.get(n.saturating_add(1)).map_or(per.len(), |&(next, _)| next);
        let mut weights = vec![Vec::new(); levels.min(N)];
        for element in per.get(first..end).unwrap_or_default() {
            for (out, &w) in weights.iter_mut().zip(element.iter()) {
                if w != W::default() {
                    put(w, out);
                }
            }
        }
        units.push(Unit { start, weights });
    }
    units
}
