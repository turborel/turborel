//! Tests of row pattern recognition: which match is preferred, and the
//! limits of a search.

mod brute;

use super::*;
use std::cell::{Cell, RefCell};

const A: usize = 0;
const B: usize = 1;
const C: usize = 2;
const D: usize = 3;

fn var(v: usize) -> Pattern {
    Pattern::Variable(v)
}

fn seq(ps: Vec<Pattern>) -> Pattern {
    Pattern::Concat(ps)
}

fn alt(ps: Vec<Pattern>) -> Pattern {
    Pattern::Alternation(ps)
}

fn rep(p: Pattern, min: u32, max: Option<u32>, reluctant: bool) -> Pattern {
    Pattern::Repeat { pattern: Box::new(p), min, max, reluctant }
}

fn name(v: usize) -> char {
    char::from(b"ABCD"[v])
}

fn letter(m: &Mapped) -> char {
    let c = name(m.variable);
    if m.excluded { c.to_ascii_lowercase() } else { c }
}

/// The match of `p` in `rows`, each the letters of the variables that hold
/// there, beginning at `start` with `first` the first row: its letters,
/// lowercase if excluded.
fn find_at(p: &Pattern, rows: &[&str], (first, start): (usize, usize)) -> Option<String> {
    let mut holds = |v: usize, path: &[Mapped]| Ok(rows[start.saturating_add(path.len()).saturating_sub(1)].contains(name(v)));
    let found = Program::new(p).unwrap().find((first, start, rows.len()), &mut holds).unwrap();
    found.map(|ms| ms.iter().map(letter).collect())
}

fn find(p: &Pattern, rows: &[&str]) -> Option<String> {
    find_at(p, rows, (0, 0))
}

/// The strings of `p` in the order tried: `p` is followed by `D`, which
/// never holds, every other variable holding, and each path `D` is tried
/// after is kept. `rows` must exceed the longest string.
fn tried(p: &Pattern, rows: usize) -> Vec<String> {
    let seen = RefCell::new(Vec::new());
    let mut holds = |v: usize, path: &[Mapped]| {
        if v == D {
            seen.borrow_mut().push(path[..path.len().saturating_sub(1)].iter().map(letter).collect::<String>());
        }
        Ok(v != D)
    };
    let program = Program::new(&seq(vec![p.clone(), var(D)])).unwrap();
    assert_eq!(program.find((0, 0, rows), &mut holds).unwrap(), None);
    seen.into_inner()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.41 GR6d, GR9a] a
/// concatenation maps each row in turn to its variables, and matches only
/// if each holds.
#[test]
fn concatenation_maps_rows_in_order() {
    let p = seq(vec![var(A), var(B), var(C)]);
    assert_eq!(find(&p, &["A", "B", "C"]), Some("ABC".to_owned()));
    assert_eq!(find(&p, &["A", "C", "C"]), None);
    assert_eq!(find(&p, &["A", "B"]), None, "no row for C");
    assert_eq!(find(&p, &["A", "B", "C", "A"]), Some("ABC".to_owned()), "rows after a match are left");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.41 GR6e] the earlier
/// alternative is preferred, the later tried if the earlier has no
/// candidate.
#[test]
fn alternation_prefers_the_earlier() {
    assert_eq!(find(&alt(vec![var(A), var(B)]), &["AB"]), Some("A".to_owned()));
    assert_eq!(find(&alt(vec![var(A), var(B)]), &["B"]), Some("B".to_owned()));
    assert_eq!(find(&alt(vec![var(A), seq(vec![var(A), var(B)])]), &["A", "B"]), Some("A".to_owned()));
    assert_eq!(find(&alt(vec![seq(vec![var(A), var(B)]), var(A)]), &["A", "B"]), Some("AB".to_owned()));
    assert_eq!(find(&alt(vec![]), &["A"]), None, "an empty alternation has no string");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.41 GR6b iii, GR6c iii;
/// §7.9 SR19] greedy quantifiers prefer more iterations and reluctant
/// fewer, in the forms `{n,m}`, `*`, `+` and `?` read as `{n,m}`. MC/DC of
/// the choice `max.is_some_and(|max| count >= max)`: `max` (`{1,3}` bounded
/// after three iterations, which ends, vs `*` unbounded after three, which
/// goes on) with `count >= max` true; `count >= max` (`{1,3}` after three
/// vs after one) with `max` bounded. The choice `count < min` is true of
/// `{2,3}` before its second iteration and false of `{0,1}`.
#[test]
fn greedy_prefers_more_and_reluctant_fewer() {
    let rows = ["A", "A", "A", "A"];
    assert_eq!(find(&rep(var(A), 1, Some(3), false), &rows), Some("AAA".to_owned()));
    assert_eq!(find(&rep(var(A), 1, Some(3), true), &rows), Some("A".to_owned()));
    assert_eq!(find(&rep(var(A), 0, None, false), &rows), Some("AAAA".to_owned()), "*");
    assert_eq!(find(&rep(var(A), 0, None, true), &rows), Some(String::new()), "*?");
    assert_eq!(find(&rep(var(A), 1, None, true), &rows), Some("A".to_owned()), "+?");
    assert_eq!(find(&rep(var(A), 0, Some(1), false), &rows), Some("A".to_owned()), "?");
    assert_eq!(find(&rep(var(A), 0, Some(1), true), &rows), Some(String::new()), "??");
    assert_eq!(find(&rep(var(A), 2, Some(3), false), &["A"]), None, "too few rows");
    assert_eq!(find(&rep(var(A), 0, Some(0), false), &rows), Some(String::new()), "{{0,0}}");
    let lazy = seq(vec![rep(var(A), 0, None, true), var(B)]);
    assert_eq!(find(&lazy, &["A", "A", "AB", "B"]), Some("AAB".to_owned()));
    let eager = seq(vec![rep(var(A), 0, None, false), var(B)]);
    assert_eq!(find(&eager, &["A", "A", "AB", "B"]), Some("AAAB".to_owned()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.22 NOTE 46; §9.41 GR6b
/// iii, GR6c iii] the standard's examples, `(A|B){1,2}` and `(A|B){1,2}?`,
/// tried in its order of preferment.
#[test]
fn the_preferment_examples_of_note_46() {
    let ab = alt(vec![var(A), var(B)]);
    assert_eq!(tried(&rep(ab.clone(), 1, Some(2), false), 3), ["AA", "AB", "A", "BA", "BB", "B"]);
    assert_eq!(tried(&rep(ab, 1, Some(2), true), 3), ["A", "AA", "AB", "B", "BA", "BB"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.41 GR6d NOTE 477] a
/// concatenation of alternations, `(A|B)(C|A)` here as D ends the test
/// pattern, is tried in the order AC, AA, BC, BA.
#[test]
fn the_preferment_example_of_note_477() {
    let p = seq(vec![alt(vec![var(A), var(B)]), alt(vec![var(C), var(A)])]);
    assert_eq!(tried(&p, 3), ["AC", "AA", "BC", "BA"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.9 SR21] PERMUTE is the
/// alternation of its permutations in lexicographic order; past
/// `MAX_PERMUTE` patterns it is SQLCODE -900.
#[test]
fn permute_tries_permutations_lexicographically() {
    let p = Pattern::Permute(vec![var(A), var(B), var(C)]);
    assert_eq!(tried(&p, 4), ["ABC", "ACB", "BAC", "BCA", "CAB", "CBA"]);
    assert_eq!(find(&Pattern::Permute(vec![var(A), var(B)]), &["B", "A"]), Some("BA".to_owned()));
    assert_eq!(find(&Pattern::Permute(vec![]), &["A"]), Some(String::new()));
    assert_eq!(compile::permutations(3), [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]]);
    let many = Pattern::Permute((0..=compile::MAX_PERMUTE).map(|_| var(A)).collect());
    assert_eq!(Program::new(&many).unwrap_err().sqlcode(), -900_i32);
    let most = Pattern::Permute((0..compile::MAX_PERMUTE).map(|_| var(A)).collect());
    assert!(Program::new(&most).is_ok());
    let nested = Pattern::Permute((0..compile::MAX_PERMUTE).map(|_| most.clone()).collect());
    assert_eq!(Program::new(&nested).unwrap_err().sqlcode(), -900_i32, "past MAX_INSTRUCTIONS");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.41 GR9b, GR9c] `^` holds
/// only at the first row and `$` only past the last.
#[test]
fn start_and_end_anchor() {
    let rows = ["A", "A", "A"];
    let start = seq(vec![Pattern::Start, var(A)]);
    assert_eq!(find_at(&start, &rows, (0, 0)), Some("A".to_owned()));
    assert_eq!(find_at(&start, &rows, (0, 1)), None);
    assert_eq!(find_at(&start, &rows, (1, 1)), Some("A".to_owned()));
    let late = seq(vec![var(A), Pattern::Start]);
    assert_eq!(find_at(&late, &rows, (0, 0)), None, "a variable precedes ^");
    let end = seq(vec![var(A), Pattern::End]);
    assert_eq!(find_at(&end, &rows, (0, 0)), None);
    assert_eq!(find_at(&end, &rows, (0, 2)), Some("A".to_owned()));
    assert_eq!(find(&seq(vec![rep(var(A), 0, None, true), Pattern::End]), &rows), Some("AAA".to_owned()));
    assert_eq!(find_at(&Pattern::End, &rows, (0, 3)), Some(String::new()), "an empty match past the last row");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.41 GR6a iv, GR7] `( )` and
/// a pattern mapping no row give an empty match, which is not no match.
#[test]
fn an_empty_match_is_a_candidate() {
    assert_eq!(find(&Pattern::Empty, &["A"]), Some(String::new()));
    assert_eq!(find(&Pattern::Empty, &[]), Some(String::new()));
    assert_eq!(find(&rep(var(A), 0, None, false), &[]), Some(String::new()));
    assert_eq!(find(&rep(var(A), 0, Some(1), false), &["B"]), Some(String::new()));
    assert_eq!(find(&var(A), &["B"]), None);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.41 GR6a vi] rows mapped
/// inside `{- -}` are marked excluded, nested or quantified too. MC/DC of
/// `self.excluded || excluded` in `Within::inner`: `self.excluded` (the
/// concatenation inside `{- -}`, already excluded, vs the outer one) with
/// `excluded` false; `excluded` (`{- B -}` vs the concatenation holding it)
/// with `self.excluded` false.
#[test]
fn excluded_rows_are_marked() {
    let ex = |p| Pattern::Excluded(Box::new(p));
    let p = seq(vec![var(A), ex(var(B)), var(C)]);
    assert_eq!(find(&p, &["A", "B", "C"]), Some("AbC".to_owned()));
    let q = seq(vec![ex(seq(vec![rep(var(B), 1, None, false), ex(var(C))])), var(A)]);
    assert_eq!(find(&q, &["B", "B", "C", "A"]), Some("bbcA".to_owned()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.41 GR9a; §4.22] a
/// variable's condition may read the rows mapped before it: B holds only
/// after a row mapped to A, so `(C|A) B` backtracks from C, judging B's row
/// on two paths.
#[test]
fn holds_reads_the_path() {
    let calls = Cell::new(0_u32);
    let mut holds = |v: usize, path: &[Mapped]| {
        if v == B {
            calls.set(calls.get().saturating_add(1));
            return Ok(path.len().checked_sub(2).is_some_and(|k| path[k].variable == A));
        }
        Ok(true)
    };
    let p = seq(vec![alt(vec![var(C), var(A)]), var(B)]);
    let found = Program::new(&p).unwrap().find((0, 0, 2), &mut holds).unwrap().unwrap();
    assert_eq!(found.iter().map(letter).collect::<String>(), "AB");
    assert_eq!(calls.get(), 2);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.41 GR6b i NOTE 473] past
/// the first n−1, only the last iteration may map no row, so a quantifier
/// of an empty-matching body ends: `(A?|B)*` on a row of B prefers one
/// empty iteration over an empty one followed by B, and `(A?|B){2,}` may
/// begin with an empty iteration. MC/DC of `empty && count >= min`: `empty`
/// (an empty iteration of `(A?|B)*`, which ends it, vs an iteration of `A*`
/// mapping a row, which goes on) with `count >= min` true; `count >= min`
/// (the empty first iteration of `(A?|B)*`, at least 0, vs that of
/// `(A?|B){2,}`, below 2, which goes on) with `empty` true.
#[test]
fn empty_iterations_end_a_quantifier() {
    let body = alt(vec![rep(var(A), 0, Some(1), false), var(B)]);
    assert_eq!(find(&rep(body.clone(), 0, None, false), &["B"]), Some(String::new()));
    assert_eq!(find(&rep(body, 2, None, false), &["B"]), Some(String::new()));
    let stars = rep(rep(rep(var(A), 0, None, false), 0, None, false), 0, None, false);
    assert_eq!(find(&stars, &["A", "A", "A", "B"]), Some("AAA".to_owned()));
    assert_eq!(find(&rep(rep(Pattern::Empty, 0, None, false), 0, None, true), &["A"]), Some(String::new()));
    let opt = rep(rep(rep(var(A), 0, Some(1), false), 3, None, false), 0, None, false);
    assert_eq!(find(&opt, &["B", "B"]), Some(String::new()));
    assert_eq!(find(&rep(Pattern::Empty, 1_000_000, None, false), &["A"]), Some(String::new()), "the first n may be empty");
}

/// Verifies: [DESIGN] a search past `MAX_STEPS` is SQLCODE -900: `(A|A)* B`
/// where B never holds tries 2 to the 40 ways.
#[test]
fn a_search_past_max_steps_fails() {
    let p = seq(vec![rep(alt(vec![var(A), var(A)]), 0, None, false), var(B)]);
    let mut holds = |v: usize, _: &[Mapped]| Ok(v == A);
    let err = Program::new(&p).unwrap().find((0, 0, 40), &mut holds).unwrap_err();
    assert_eq!(err.sqlcode(), -900_i32);
    assert!(err.msg.contains("steps"));
}

/// Verifies: [DESIGN] patterns nested past `MAX_DEPTH` are SQLCODE -900.
#[test]
fn a_pattern_nested_too_deep_fails() {
    let mut p = var(A);
    for _ in 0..=compile::MAX_DEPTH {
        p = Pattern::Excluded(Box::new(p));
    }
    assert_eq!(Program::new(&p).unwrap_err().sqlcode(), -900_i32);
    let mut q = var(A);
    for _ in 0..compile::MAX_DEPTH {
        q = Pattern::Excluded(Box::new(q));
    }
    assert_eq!(Program::new(&q).unwrap().pattern(), &q);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.41 GR9a] an error judging
/// a row ends the search with that error.
#[test]
fn an_error_of_holds_is_returned() {
    let mut holds = |v: usize, _: &[Mapped]| if v == B { Err(SqlError::new(Code::Data, "boom")) } else { Ok(true) };
    let p = alt(vec![seq(vec![var(A), var(B)]), var(A)]);
    let err = Program::new(&p).unwrap().find((0, 0, 2), &mut holds).unwrap_err();
    assert_eq!(err.code, Code::Data);
}
