//! \[DESIGN\] What a thread keeps beside its bindings (RULES.md D142):
//! the path so far, for a prefilter's path function [GS §6.9], and the
//! elements of an element pattern whose prefilter reads a variable bound
//! after it, tested as that variable is bound [GS §6.8].
//!
//! A deferred element is kept with a snapshot of what its prefilter
//! reads as it is bound: the path's length so far, and the last element
//! of each register the prefilter reads, or a sign of none. When the
//! variables it waits for are all bound, each element kept is tested,
//! the snapshot read for the rest; one false ends the thread. An element
//! still kept as its waiting scope begins again, or as the pattern is
//! matched, is tested with what it waits for unbound, as a null.

use super::{Runner, Thread, bind};
use crate::error::Result;
use crate::graph::nfa::Test;
use crate::graph::program::{Bound, Resolved, absent, is_absent, marked};
use crate::graph::view::{ElementId, TableId};

/// The registers of a read a snapshot keeps: its own, PREV and FIRST.
const KEPT: usize = 3;

/// The places of a kept element's entry before its reads': the element
/// and the path's length.
const HEAD: usize = 2;

impl Runner<'_> {
    /// `thread` with `element` bound by `test`, none if a join fails: its
    /// path register extended, and its element kept if its prefilter is
    /// deferred.
    pub(super) fn bound(&self, test: &Test, thread: &Thread, element: ElementId) -> Option<Thread> {
        let mut bound = bind(test, thread, element)?;
        if let Some(k) = test.defer {
            let pending = self.nfa.pending.get(k)?;
            let length = pending.filter.path.and_then(|r| thread.registers.get(r)).map_or(0, |p| p.len());
            let mut entry = vec![element, ElementId { table: TableId::MAX, row: u64::try_from(length).unwrap_or(u64::MAX) }];
            for read in &pending.filter.reads {
                for register in [read.1, read.2, read.3] {
                    let last = register.and_then(|r| thread.registers.get(r)).and_then(|held| held.last().copied());
                    entry.push(last.unwrap_or_else(absent));
                }
            }
            let held = bound.registers.get_mut(pending.register)?;
            *held = held.iter().copied().chain(entry).collect();
        }
        if let Some((register, mark)) = test.path {
            let held = bound.registers.get_mut(register)?;
            let last = held.iter().rev().find(|e| !is_absent(**e)).copied();
            match test.var.filter(|_| mark) {
                Some(var) => *held = held.iter().copied().chain([marked(var)]).collect(),
                None => {
                    if last != Some(element) {
                        *held = held.iter().copied().chain([element]).collect();
                    }
                }
            }
        }
        Some(bound)
    }

    /// `thread` after `test` bound its variable, the deferred prefilters
    /// that waited for it tested once all they wait for is bound; none if
    /// one is false.
    ///
    /// # Errors
    ///
    /// Any error of a prefilter.
    pub(super) fn triggered(&self, test: &Test, mut thread: Thread) -> Result<Option<Thread>> {
        // A search run from the path's end may have bound them already.
        for &k in test.defer.iter().chain(&test.triggers) {
            if !self.deferred(k, &mut thread, false)? {
                return Ok(None);
            }
        }
        Ok(Some(thread))
    }

    /// Whether the elements kept for the deferred prefilters waiting in
    /// `scope` hold, what they wait for unbound, as it begins again.
    ///
    /// # Errors
    ///
    /// Any error of a prefilter.
    pub(super) fn flushed(&self, scope: usize, thread: &mut Thread) -> Result<bool> {
        let flushes = self.nfa.scopes.get(scope).map(|s| s.flushes.clone()).unwrap_or_default();
        for k in flushes {
            if !self.deferred(k, thread, true)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Whether every element still kept holds as the pattern is matched.
    ///
    /// # Errors
    ///
    /// Any error of a prefilter.
    pub(super) fn settled(&self, thread: &mut Thread) -> Result<bool> {
        for k in 0..self.nfa.pending.len() {
            if !self.deferred(k, thread, true)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Tests the elements kept for deferred prefilter `k`, if all it waits
    /// for is bound or `flush`, then forgets them; whether each held.
    fn deferred(&self, k: usize, thread: &mut Thread, flush: bool) -> Result<bool> {
        let Some(p) = self.nfa.pending.get(k) else { return Ok(true) };
        let waited = p.filter.reads.iter().zip(&p.later).filter(|(_, later)| **later).map(|(r, _)| r.1);
        let ready = flush || waited.into_iter().all(|r| r.and_then(|r| thread.registers.get(r)).is_some_and(|h| !h.is_empty()));
        let kept = thread.registers.get(p.register).cloned().unwrap_or_default();
        if !ready || kept.is_empty() {
            return Ok(true);
        }
        let width = p.filter.reads.len().saturating_mul(KEPT).saturating_add(HEAD);
        let base = thread.registers.len();
        for entry in kept.chunks(width) {
            let mut registers = thread.registers.to_vec();
            registers.extend(entry.iter().skip(HEAD).map(|e| Box::from([*e])));
            let resolved: Vec<Resolved> = p
                .filter
                .reads
                .iter()
                .zip(&p.later)
                .enumerate()
                .map(|(i, (read, later))| {
                    let at = |n: usize, r: Option<usize>| r.map(|_| base.saturating_add(i.saturating_mul(KEPT)).saturating_add(n));
                    match (*later, flush) {
                        (true, true) => (read.0, None, None, None),
                        (true, false) => *read,
                        (false, _) => (read.0, at(0, read.1), at(1, read.2), at(2, read.3)),
                    }
                })
                .collect();
            let length = entry.get(1).and_then(|e| usize::try_from(e.row).ok()).unwrap_or(0);
            let whole = p.filter.path.and_then(|r| thread.registers.get(r)).map_or(&[][..], |h| &**h);
            let path = whole.get(..length).unwrap_or(whole);
            let element = entry.first().map(|e| (p.own, *e));
            if !(p.filter.test)(&Bound { element, registers: &registers, reads: &resolved, path })? {
                return Ok(false);
            }
        }
        if let Some(held) = thread.registers.get_mut(p.register) {
            *held = Box::default();
        }
        Ok(true)
    }
}
