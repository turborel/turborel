//! \[EXTENSION\] Each path pattern searched, those whose prefilters read
//! another path pattern's bindings after that one [GS §6.8] (RULES.md
//! D142).
//!
//! A prefilter is applied before its path pattern's selector, so a
//! variable of another path pattern it reads must be bound as it is
//! tested. That path pattern is searched first, and this one once for
//! each choice of the bindings it reads, those fixed for its prefilters.
//! Each binding found is tagged with the choice, and a match takes it
//! only beside the bindings it was searched with.

use std::sync::Arc;

use super::{Rows, reach, shared, start};
use crate::error::Result;
use crate::graph::binding::PathBinding;
use crate::graph::program::PathPattern;
use crate::graph::search::{PathSearch, SearchOptions};
use crate::graph::view::ElementId;
use crate::logical::{GraphMatch, PathShape};
use shared::Shared;

/// A path pattern searched: as searched, its bindings, and each one's
/// choice of the bindings of the path patterns it read, by number.
pub(super) type Searched = (PathPattern, Vec<PathBinding>, Vec<Vec<(usize, usize)>>);

/// The order the path patterns of `g` are searched in: each after those
/// its prefilters read, else as written.
pub(super) fn order(g: &GraphMatch) -> Vec<usize> {
    let count = g.paths.len();
    let depends: Vec<Vec<usize>> = (0..count).map(|n| g.depends(n)).collect();
    let mut done: Vec<usize> = Vec::with_capacity(count);
    while done.len() < count {
        let ready = (0..count).find(|n| !done.contains(n) && depends.get(*n).is_none_or(|d| d.iter().all(|m| done.contains(m))));
        // A cycle the binder refuses never comes here; written order then.
        let next = ready.or_else(|| (0..count).find(|n| !done.contains(n)));
        let Some(next) = next else { break };
        done.push(next);
    }
    done
}

/// Each choice of one of `counts` things of each kind, in order.
fn choices(counts: &[usize]) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new()];
    for &count in counts {
        out = out.into_iter().flat_map(|c| (0..count).map(move |i| c.iter().copied().chain([i]).collect())).collect();
    }
    out
}

impl Rows {
    /// Path pattern `n`, as `path` searches it, from `starts`: of one
    /// whose prefilters read others' bindings, once for each choice of
    /// those of `out`; of one written as an earlier one, `earlier`, its
    /// bindings.
    ///
    /// # Errors
    ///
    /// Any error of the search.
    pub(super) fn path_searched(
        &self,
        shared: &Arc<Shared>,
        (n, path, reversed): (usize, &PathShape, bool),
        (starts, options, earlier): (&[ElementId], &SearchOptions, Option<usize>),
        out: &[Option<Searched>],
    ) -> Result<Searched> {
        let depends = self.graph.depends(n);
        if depends.is_empty() {
            // [GS §16 R16.6] A path pattern written as an earlier one is not searched again.
            let taken = earlier.filter(|&m| self.graph.depends(m).is_empty()).and_then(|m| out.get(m).cloned().flatten());
            if let Some(found) = taken {
                return Ok(found);
            }
            let (pattern, bindings) = self.search_one(shared, (n, path, reversed), starts, options)?;
            return Ok((pattern, bindings, Vec::new()));
        }
        let read = |m: usize| out.get(m).and_then(Option::as_ref).map(|s| s.1.as_slice()).unwrap_or_default();
        let counts: Vec<usize> = depends.iter().map(|&m| read(m).len()).collect();
        let (mut pattern, mut bindings, mut tags) = (None, Vec::new(), Vec::new());
        for choice in choices(&counts) {
            let mut fixed = vec![None; self.graph.paths.len()];
            for (&m, &i) in depends.iter().zip(&choice) {
                if let Some(slot) = fixed.get_mut(m) {
                    *slot = read(m).get(i).cloned();
                }
            }
            let each = Arc::new(shared.with_fixed(fixed));
            let (p, found) = self.search_one(&each, (n, path, reversed), starts, options)?;
            let tag: Vec<(usize, usize)> = depends.iter().copied().zip(choice).collect();
            tags.extend(std::iter::repeat_n(tag, found.len()));
            bindings.extend(found);
            pattern.get_or_insert(p);
        }
        let pattern = pattern.unwrap_or_else(|| path.instantiate(&mut |k| shared::filter(shared, k)));
        Ok((pattern, bindings, tags))
    }

    /// The bindings of one search of path pattern `n` from `starts`.
    fn search_one(
        &self,
        shared: &Arc<Shared>,
        (n, path, reversed): (usize, &PathShape, bool),
        starts: &[ElementId],
        options: &SearchOptions,
    ) -> Result<(PathPattern, Vec<PathBinding>)> {
        let mut pattern = path.instantiate(&mut |k| shared::filter(shared, k));
        if path.cost.is_some() {
            pattern = pattern.cost(shared::cost(shared, n));
        }
        let step_edges = self.graph.sizing.step(n);
        let search = PathSearch::new(&pattern, &*shared.store, SearchOptions { step_edges, ..options.clone() })?;
        let pruned = reach::pruned(shared, path, starts)?;
        // The run raises 01W61 itself as a truncated search ends.
        let bindings: Vec<PathBinding> = search.run(&pruned, None)?.collect::<Result<_>>()?;
        let bindings = start::forward(bindings, reversed);
        drop(search);
        Ok((pattern, bindings))
    }
}

/// Whether `choice`, a binding of each path pattern, takes each binding
/// beside those it was searched with.
pub(super) fn consistent(tags: &[Vec<Vec<(usize, usize)>>], choice: &[usize]) -> bool {
    tags.iter().zip(choice).all(|(t, &c)| t.get(c).is_none_or(|read| read.iter().all(|&(m, i)| choice.get(m) == Some(&i))))
}
