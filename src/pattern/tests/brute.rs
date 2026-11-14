//! A cross-check of the search against the standard's definitions read
//! naively: the parenthesized language of a small pattern written out in
//! full, ordered by the preferment rules as printed, and its first
//! candidate taken.

use super::super::*;
use std::cmp::Ordering;

/// A symbol of an identifier string that the candidate rules read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sym {
    Var(usize, bool),
    Start,
    End,
}

type Str = Vec<Sym>;

fn vcount(s: &[Sym]) -> usize {
    s.iter().filter(|x| matches!(x, Sym::Var(..))).count()
}

/// PL(p) in order of preferment [§9.41 GR6], leaving out strings of more
/// than `budget` variables, which cannot be candidates.
fn language(p: &Pattern, budget: usize, excluded: bool) -> Vec<Str> {
    let mut all = match p {
        Pattern::Variable(v) => vec![vec![Sym::Var(*v, excluded)]],
        Pattern::Start => vec![vec![Sym::Start]],
        Pattern::End => vec![vec![Sym::End]],
        Pattern::Empty => vec![vec![]],
        Pattern::Concat(ps) => concat(ps.iter().collect(), budget, excluded),
        Pattern::Alternation(ps) => ps.iter().flat_map(|q| language(q, budget, excluded)).collect(),
        Pattern::Excluded(q) => language(q, budget, true),
        Pattern::Permute(ps) => {
            orders(ps.len()).iter().flat_map(|order| concat(order.iter().map(|&i| &ps[i]).collect(), budget, excluded)).collect()
        }
        Pattern::Repeat { pattern, min, max, reluctant } => repeat(&language(pattern, budget, excluded), (*min, *max, *reluctant), budget),
    };
    all.retain(|s| vcount(s) <= budget);
    all
}

/// [§9.41 GR6d] Lexicographic in the operands.
fn concat(ps: Vec<&Pattern>, budget: usize, excluded: bool) -> Vec<Str> {
    let mut acc: Vec<Str> = vec![vec![]];
    for p in ps {
        let next = language(p, budget, excluded);
        acc = acc.iter().flat_map(|a| next.iter().map(move |b| [a.clone(), b.clone()].concat())).filter(|s| vcount(s) <= budget).collect();
    }
    acc
}

/// [§7.9 SR21] Permutations of `0..n` in lexicographic order, the least
/// unused index chosen first at each place.
fn orders(n: usize) -> Vec<Vec<usize>> {
    fn grow(prefix: &mut Vec<usize>, n: usize, out: &mut Vec<Vec<usize>>) {
        if prefix.len() == n {
            out.push(prefix.clone());
        }
        for i in 0..n {
            if !prefix.contains(&i) {
                prefix.push(i);
                grow(prefix, n, out);
                prefix.pop();
            }
        }
    }
    let mut out = Vec::new();
    grow(&mut Vec::new(), n, &mut out);
    out
}

/// [§9.41 GR6b, GR6c] Every sequence of iterations allowed, as indexes into
/// `sub`, sorted by the comparison rule as printed.
fn repeat(sub: &[Str], (min, max, reluctant): (u32, Option<u32>, bool), budget: usize) -> Vec<Str> {
    let n = usize::try_from(min).unwrap();
    let nn = n.saturating_sub(1);
    let most = max.map_or(usize::MAX, |m| usize::try_from(m).unwrap()).min(nn.saturating_add(budget).saturating_add(1));
    let mut seqs = Vec::new();
    let mut prefix = Vec::new();
    iterations(sub, (n, nn, most), budget, &mut prefix, &mut seqs);
    seqs.sort_by(|a, b| prefer(a, b, reluctant));
    seqs.iter().map(|s| s.iter().flat_map(|&i| sub[i].clone()).collect()).collect()
}

fn iterations(sub: &[Str], (n, nn, most): (usize, usize, usize), left: usize, prefix: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    let p = prefix.len();
    let middle = |k: usize| nn < k.saturating_add(1) && k.saturating_add(1) < p;
    let middle_ok = prefix.iter().enumerate().all(|(k, &i)| !middle(k) || vcount(&sub[i]) > 0);
    if !middle_ok {
        return;
    }
    if p >= n && p <= most {
        out.push(prefix.clone());
    }
    if p >= most {
        return;
    }
    for (i, s) in sub.iter().enumerate() {
        if vcount(s) <= left {
            prefix.push(i);
            iterations(sub, (n, nn, most), left.saturating_sub(vcount(s)), prefix, out);
            prefix.pop();
        }
    }
}

/// Less if `a` is preferred over `b`.
fn prefer(a: &[usize], b: &[usize], reluctant: bool) -> Ordering {
    let common = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let (alen, blen) = (a.len(), b.len());
    let a_first = if common < alen && common < blen {
        a[common] < b[common]
    } else if reluctant {
        common == alen && alen < blen
    } else {
        common == blen && blen < alen
    };
    if a == b {
        Ordering::Equal
    } else if a_first {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

/// [§9.41 GR9, GR10] The mapping of the first candidate of `strings`.
fn first_candidate(
    strings: &[Str],
    (first, start, end): (usize, usize, usize),
    holds: &impl Fn(usize, &[Mapped]) -> bool,
) -> Option<Vec<Mapped>> {
    'strings: for s in strings {
        let mut path = Vec::new();
        for sym in s {
            let pos = start.saturating_add(path.len());
            let ok = match *sym {
                Sym::Start => pos == first,
                Sym::End => pos == end,
                Sym::Var(variable, excluded) => {
                    path.push(Mapped { variable, excluded });
                    pos < end && holds(variable, &path)
                }
            };
            if !ok {
                continue 'strings;
            }
        }
        return Some(path);
    }
    None
}

/// A fixed-seed generator of test cases.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> usize {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        usize::try_from((self.0 >> 33).checked_rem(n).unwrap()).unwrap()
    }
}

fn random_pattern(g: &mut Lcg, depth: u32) -> Pattern {
    let leaf = depth == 0 || g.below(3) == 0;
    if leaf {
        return match g.below(10) {
            0 => Pattern::Start,
            1 => Pattern::End,
            2 => Pattern::Empty,
            _ => Pattern::Variable(g.below(3)),
        };
    }
    let inner = depth.saturating_sub(1);
    let operands = |r: &mut Lcg| (0..operand_count(r)).map(|_| random_pattern(r, inner)).collect::<Vec<_>>();
    match g.below(5) {
        0 => Pattern::Concat(operands(g)),
        1 => Pattern::Alternation(operands(g)),
        2 => Pattern::Excluded(Box::new(random_pattern(g, inner))),
        3 => Pattern::Permute(operands(g)),
        _ => {
            let min = u32::try_from(g.below(3)).unwrap();
            let max = if g.below(2) == 0 { None } else { Some(min.saturating_add(u32::try_from(g.below(2)).unwrap())) };
            Pattern::Repeat { pattern: Box::new(random_pattern(g, inner)), min, max, reluctant: g.below(2) == 0 }
        }
    }
}

/// Two or three operands.
fn operand_count(g: &mut Lcg) -> usize {
    g.below(2).saturating_add(2)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.41 GR6 to GR10] on random
/// small patterns, from each row of a short sequence, the search finds what
/// the first candidate of the parenthesized language, written out and
/// ordered by the preferment rules as printed, maps, with conditions that
/// read the previous row's variable.
#[test]
fn the_search_agrees_with_the_language_in_preferment_order() {
    let mut g = Lcg(0x5eed);
    let rows = 3usize;
    for case in 0..3000_u32 {
        let p = random_pattern(&mut g, 3);
        let table: Vec<bool> = (0..64_u32).map(|_| g.below(3) != 0).collect();
        let holds = |v: usize, path: &[Mapped]| {
            let prev = path.len().checked_sub(2).map_or(0, |k| path[k].variable.saturating_add(1));
            table[v.saturating_mul(16).saturating_add(prev.saturating_mul(4)).saturating_add(path.len().min(3))]
        };
        let program = Program::new(&p).unwrap();
        for start in 0..=rows {
            let budget = rows.saturating_sub(start);
            let expected = first_candidate(&language(&p, budget, false), (0, start, rows), &holds);
            let mut search_holds = |v: usize, path: &[Mapped]| Ok(holds(v, path));
            let found = program.find((0, start, rows), &mut search_holds).unwrap();
            assert_eq!(found, expected, "case {case}, start {start}, pattern {p:?}");
        }
    }
}
