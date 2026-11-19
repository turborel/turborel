//! \[DESIGN\] Which bindings a thread of the automaton keeps, and the
//! termination rules checked as they are chosen [GS §7.4].
//!
//! A variable repeated within one iteration, or within the path outside
//! any quantifier, joins on itself [GS §6.4] [P1 §6.4]: its register holds
//! its first binding and a later one must equal it. A prefilter reads a
//! variable through the register of the innermost scope holding both the
//! prefilter and every place the variable is bound, a group if a
//! quantifier lies between [P1 §4.4]. `PREV(x)` reads x's binding in the
//! previous iteration of the quantifier around the prefilter [GS §11.1
//! R11.2], and `FIRST(x)` its binding in the first. Only what is read or joined on is kept, so a thread of a
//! pattern with neither is an instruction alone.

use std::collections::HashMap;

use super::{Compiled, Pending, Scope, ScopeKind, Test};
use crate::error::Result;
use crate::graph::program::{ElementTest, Filter, Read, Restrictor, Selector, VarId, WHOLE_PATH, termination};

/// The tests and scopes compiled, the restricted scopes' slots and the
/// registers, and the deferred prefilters.
pub(super) type Finished = (Vec<Test>, Vec<Scope>, (usize, usize), Vec<Pending>);

/// A numbered scope: its parent, kind, restrictor and prefilter.
type Numbered = (Option<usize>, ScopeKind, Restrictor, Option<Filter>);

/// The registers chosen for a pattern.
pub(super) struct Registers {
    /// Each register by its scope and variable, and whether it joins.
    current: HashMap<(usize, VarId), (usize, bool)>,
    /// Each PREV register by its quantifier and variable, and its source.
    previous: HashMap<(usize, VarId), (usize, usize)>,
    /// Each FIRST register by its quantifier and variable, and its source.
    first: HashMap<(usize, VarId), (usize, usize)>,
    count: usize,
    /// The parent of each scope.
    parents: Vec<Option<usize>>,
    /// The path register, if a prefilter reads the path so far.
    path: Option<usize>,
    /// The subpath marks a prefilter's path function reads.
    marks: Vec<VarId>,
    /// Each deferred prefilter: its test, its register and the scope
    /// holding it and every variable it waits for.
    pending: Vec<(usize, usize, usize)>,
}

impl Registers {
    /// Chooses the registers of the scopes and tests numbered, checking
    /// GRF-0031 and GRF-0032 [GS §7.4 R7.3, R7.4].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if either rule is broken.
    pub(super) fn allocate(numbering: &super::Numbering, selector: Selector) -> Result<Registers> {
        let parents = numbering.scopes.iter().map(|s| s.0).collect();
        let mut chosen = Registers {
            current: HashMap::new(),
            previous: HashMap::new(),
            first: HashMap::new(),
            count: 0,
            parents,
            path: None,
            marks: Vec::new(),
            pending: Vec::new(),
        };
        chosen.unbounded(&numbering.scopes, selector)?;
        chosen.joins(&numbering.tests, &numbering.scopes);
        let sites = sites(&numbering.tests);
        for (k, (scope, test, _)) in numbering.tests.iter().enumerate() {
            if let Some(filter) = &test.filter {
                chosen.reads(filter, (*scope, test.var), &sites, &numbering.scopes)?;
                chosen.deferred((k, test.var), filter, *scope, (&sites, &numbering.scopes))?;
            }
        }
        for (scope, numbered) in numbering.scopes.iter().enumerate() {
            if let Some(filter) = &numbered.3 {
                chosen.reads(filter, (scope, None), &sites, &numbering.scopes)?;
            }
        }
        Ok(chosen)
    }

    /// `scope` and its ancestors, innermost first.
    fn ancestors(&self, scope: usize) -> Vec<usize> {
        let mut chain = vec![scope];
        while let Some(parent) = chain.last().and_then(|&s| self.parents.get(s).copied().flatten()) {
            chain.push(parent);
        }
        chain
    }

    /// A new register.
    fn register(&mut self) -> usize {
        self.count = self.count.saturating_add(1);
        self.count.saturating_sub(1)
    }

    /// \[GS §7.4 R7.3\] Every unbounded quantifier is within a restrictor
    /// other than WALK, or there is a selector.
    fn unbounded(&self, scopes: &[Numbered], selector: Selector) -> Result<()> {
        for (scope, numbered) in scopes.iter().enumerate() {
            let open = numbered.1 == (ScopeKind::Quantifier { bounded: false });
            if open && selector == Selector::All && !self.restricted(scope, scopes) {
                return Err(termination("R7.3", "GRF-0031", "an unbounded quantifier is within no restrictor or selector"));
            }
        }
        Ok(())
    }

    /// Whether `scope` is within a restrictor other than WALK.
    fn restricted(&self, scope: usize, scopes: &[Numbered]) -> bool {
        self.ancestors(scope).iter().any(|&s| scopes.get(s).is_some_and(|n| n.2 != Restrictor::Walk))
    }

    /// The innermost iteration or path around `scope`, where its
    /// bindings are singletons.
    fn singleton_scope(&self, scope: usize, scopes: &[Numbered]) -> usize {
        let chain = self.ancestors(scope);
        let found = chain.iter().find(|&&s| scopes.get(s).is_some_and(|n| matches!(n.1, ScopeKind::Iteration | ScopeKind::Path)));
        found.copied().unwrap_or(scope)
    }

    /// \[GS §6.4\] A register joining each variable bound twice or more
    /// as a singleton of one scope.
    fn joins(&mut self, tests: &[(usize, ElementTest, bool)], scopes: &[Numbered]) {
        let mut counts: HashMap<(usize, VarId), usize> = HashMap::new();
        for (scope, test, _) in tests {
            if let Some(var) = test.var {
                let count = counts.entry((self.singleton_scope(*scope, scopes), var)).or_insert(0);
                *count = count.saturating_add(1);
            }
        }
        let mut repeated: Vec<(usize, VarId)> = counts.into_iter().filter(|&(_, n)| n > 1).map(|(key, _)| key).collect();
        repeated.sort_unstable();
        for key in repeated {
            let register = self.register();
            self.current.insert(key, (register, true));
        }
    }

    /// The registers a prefilter of `scope` reads, its element pattern's
    /// variable `own` none: read as the element tested.
    fn reads(
        &mut self,
        filter: &Filter,
        (scope, own): (usize, Option<VarId>),
        sites: &HashMap<VarId, Vec<usize>>,
        scopes: &[Numbered],
    ) -> Result<()> {
        for read in &filter.reads {
            let bound = sites.get(&read.var).map_or(&[][..], Vec::as_slice);
            if let Some(end) = read.path {
                self.path_of(read.var, end, scopes)?;
            } else if read.previous || read.first {
                self.iteration_of(*read, scope, scopes);
            } else if own != Some(read.var) {
                let holder = self.holder(scope, bound);
                self.bounded(read.var, holder, bound, scopes)?;
                if !self.current.contains_key(&(holder, read.var)) {
                    let register = self.register();
                    self.current.insert((holder, read.var), (register, false));
                }
            }
        }
        Ok(())
    }

    /// \[GS §6.9\] The path register, for a path function of a prefilter
    /// reading the path so far, or the subpath between marks `start` and
    /// `end`. The path is kept as a group of every element, so no
    /// unbounded quantifier may lie outside every restrictor [GS §7.4
    /// R7.4].
    fn path_of(&mut self, start: VarId, end: VarId, scopes: &[Numbered]) -> Result<()> {
        for (scope, numbered) in scopes.iter().enumerate() {
            if numbered.1 == (ScopeKind::Quantifier { bounded: false }) && !self.restricted(scope, scopes) {
                return Err(termination("R7.4", "GRF-0032", "a prefilter's path function reads a path unbounded where it reads it"));
            }
        }
        if self.path.is_none() {
            self.path = Some(self.register());
        }
        self.marks.extend([start, end].into_iter().filter(|&m| m != WHOLE_PATH));
        Ok(())
    }

    /// \[GS §6.8\] A register keeping the elements of test `k`, of
    /// variable `own`, whose prefilter reads a variable bound after it,
    /// until those are bound. What it keeps crosses no unbounded
    /// quantifier outside every restrictor [GS §7.4 R7.4].
    fn deferred(
        &mut self,
        (k, own): (usize, Option<VarId>),
        filter: &Filter,
        scope: usize,
        (sites, scopes): (&HashMap<VarId, Vec<usize>>, &[Numbered]),
    ) -> Result<()> {
        if !filter.reads.iter().any(|r| r.later) {
            return Ok(());
        }
        let waits: Vec<usize> =
            filter.reads.iter().filter(|r| r.later).flat_map(|r| sites.get(&r.var).cloned().unwrap_or_default()).collect();
        let holder = self.holder(scope, &waits);
        self.bounded(own.unwrap_or(WHOLE_PATH), holder, &[scope], scopes)?;
        let register = self.register();
        self.pending.push((k, register, holder));
        Ok(())
    }

    /// The innermost scope around `scope` and every one of `sites`.
    fn holder(&self, scope: usize, sites: &[usize]) -> usize {
        let chains: Vec<Vec<usize>> = sites.iter().map(|&s| self.ancestors(s)).collect();
        let outer = self.ancestors(scope);
        outer.iter().copied().find(|s| chains.iter().all(|chain| chain.contains(s))).unwrap_or(0)
    }

    /// \[GS §7.4 R7.4\] A prefilter reading `var` through the register of
    /// `holder` crosses no unbounded quantifier outside every restrictor.
    fn bounded(&self, var: VarId, holder: usize, sites: &[usize], scopes: &[Numbered]) -> Result<()> {
        for &site in sites {
            for scope in self.ancestors(site).into_iter().take_while(|&s| s != holder) {
                let open = scopes.get(scope).is_some_and(|n| n.1 == ScopeKind::Quantifier { bounded: false });
                if open && !self.restricted(scope, scopes) {
                    return Err(termination(
                        "R7.4",
                        "GRF-0032",
                        &format!("a prefilter reads group variable v{var}, unbounded where it reads it"),
                    ));
                }
            }
        }
        Ok(())
    }

    /// \[GS §11.1 R11.2\] The PREV or FIRST register of `read`'s variable
    /// for a prefilter of `scope`: kept by the innermost quantifier around
    /// it, set from the register of the variable in its iteration as each
    /// iteration begins, FIRST's only if not set yet.
    fn iteration_of(&mut self, read: Read, scope: usize, scopes: &[Numbered]) {
        let var = read.var;
        let chain = self.ancestors(scope);
        let at = chain.iter().position(|&s| scopes.get(s).is_some_and(|n| matches!(n.1, ScopeKind::Quantifier { .. })));
        let (Some(at), Some(iteration)) = (at, at.and_then(|a| a.checked_sub(1)).and_then(|i| chain.get(i).copied())) else { return };
        let Some(&quantifier) = chain.get(at) else { return };
        let kept = if read.first { &self.first } else { &self.previous };
        if kept.contains_key(&(quantifier, var)) {
            return;
        }
        let source = if let Some(&(register, _)) = self.current.get(&(iteration, var)) {
            register
        } else {
            let register = self.register();
            self.current.insert((iteration, var), (register, false));
            register
        };
        let register = self.register();
        let held = if read.first { &mut self.first } else { &mut self.previous };
        held.insert((quantifier, var), (register, source));
    }

    /// A prefilter with its reads resolved, of a pattern of `scope`.
    fn compiled(&self, filter: &Filter, scope: usize, sites: &HashMap<VarId, Vec<usize>>) -> Compiled {
        let chain = self.ancestors(scope);
        let reads = filter
            .reads
            .iter()
            .map(|read| {
                let holder = self.holder(scope, sites.get(&read.var).map_or(&[][..], Vec::as_slice));
                let current = self.current.get(&(holder, read.var)).map(|r| r.0);
                let previous = chain.iter().find_map(|&q| self.previous.get(&(q, read.var))).map(|r| r.0);
                let first = chain.iter().find_map(|&q| self.first.get(&(q, read.var))).copied();
                let current = if read.first { first.map(|r| r.1) } else { current.filter(|_| !read.previous) };
                (read.var, current, previous.filter(|_| read.previous), first.map(|r| r.0).filter(|_| read.first))
            })
            .collect();
        Compiled { test: filter.test.clone(), reads, path: self.path }
    }

    /// The tests and scopes compiled with their registers, the restricted
    /// scopes' slots, and the number of registers.
    pub(super) fn finish(&self, tests: &[(usize, ElementTest, bool)], scopes: &[Numbered]) -> Finished {
        let sites = sites(tests);
        let mut slots = HashMap::new();
        for (scope, numbered) in scopes.iter().enumerate() {
            if numbered.2 != Restrictor::Walk {
                let next = slots.len();
                slots.insert(scope, next);
            }
        }
        let mut pending = Vec::new();
        let mut waits = Vec::new();
        for &(k, register, _) in &self.pending {
            let Some((scope, test, _)) = tests.get(k) else { continue };
            let Some(filter) = &test.filter else { continue };
            waits.push(filter.reads.iter().filter(|r| r.later).map(|r| r.var).collect::<Vec<_>>());
            let later = filter.reads.iter().map(|r| r.later).collect();
            pending.push(Pending { register, filter: self.compiled(filter, *scope, &sites), own: test.var, later });
        }
        let compiled_tests = tests
            .iter()
            .enumerate()
            .map(|(k, (scope, test, edge))| {
                let mut t = self.test(*scope, test, *edge, (&sites, &slots));
                t.defer = self.pending.iter().position(|p| p.0 == k);
                t.triggers = waits.iter().enumerate().filter(|(_, w)| test.var.is_some_and(|v| w.contains(&v))).map(|(i, _)| i).collect();
                t
            })
            .collect();
        let compiled_scopes = scopes
            .iter()
            .enumerate()
            .map(|(scope, numbered)| {
                let mut s = self.scope(scope, numbered, (&sites, &slots));
                s.flushes = self.pending.iter().enumerate().filter(|(_, p)| p.2 == scope).map(|(i, _)| i).collect();
                s
            })
            .collect();
        (compiled_tests, compiled_scopes, (slots.len(), self.count), pending)
    }

    /// A test compiled: the registers its binding goes to, and the
    /// restricted scopes an edge's is within.
    fn test(
        &self,
        scope: usize,
        test: &ElementTest,
        edge: bool,
        (sites, slots): (&HashMap<VarId, Vec<usize>>, &HashMap<usize, usize>),
    ) -> Test {
        let chain = self.ancestors(scope);
        let binds = test.var.map_or_else(Vec::new, |var| chain.iter().filter_map(|&s| self.current.get(&(s, var)).copied()).collect());
        let restricted = if edge { chain.iter().filter_map(|s| slots.get(s).copied()).collect() } else { Vec::new() };
        let filter = test.filter.as_ref().map(|f| self.compiled(f, scope, sites));
        let path = self.path.map(|r| (r, test.var.is_some_and(|v| self.marks.contains(&v))));
        Test { var: test.var, labels: test.labels.clone(), filter, binds, slots: restricted, path, defer: None, triggers: Vec::new() }
    }

    /// A scope compiled: what it clears and sets as it begins.
    fn scope(&self, scope: usize, numbered: &Numbered, (sites, slots): (&HashMap<VarId, Vec<usize>>, &HashMap<usize, usize>)) -> Scope {
        let iterations: Vec<usize> = (0..self.parents.len()).filter(|&s| self.parents.get(s).copied().flatten() == Some(scope)).collect();
        let owned = |s: &usize| *s == scope || (matches!(numbered.1, ScopeKind::Quantifier { .. }) && iterations.contains(s));
        let mut clears: Vec<usize> = self.current.iter().filter(|((s, _), _)| owned(s)).map(|(_, r)| r.0).collect();
        clears.extend(self.previous.iter().chain(&self.first).filter(|((s, _), _)| *s == scope).map(|(_, r)| r.0));
        clears.sort_unstable();
        let mut previous: Vec<(usize, usize)> = match (numbered.1, numbered.0) {
            (ScopeKind::Iteration, Some(q)) => self.previous.iter().filter(|((s, _), _)| *s == q).map(|(_, r)| *r).collect(),
            (ScopeKind::Iteration | ScopeKind::Path | ScopeKind::Group | ScopeKind::Quantifier { .. }, _) => Vec::new(),
        };
        previous.sort_unstable();
        let mut first: Vec<(usize, usize)> = match (numbered.1, numbered.0) {
            (ScopeKind::Iteration, Some(q)) => self.first.iter().filter(|((s, _), _)| *s == q).map(|(_, r)| *r).collect(),
            (ScopeKind::Iteration | ScopeKind::Path | ScopeKind::Group | ScopeKind::Quantifier { .. }, _) => Vec::new(),
        };
        first.sort_unstable();
        let filter = numbered.3.as_ref().map(|f| self.compiled(f, scope, sites));
        Scope { restrictor: numbered.2, slot: slots.get(&scope).copied(), filter, clears, previous, first, flushes: Vec::new() }
    }
}

/// The scopes each variable is bound in.
fn sites(tests: &[(usize, ElementTest, bool)]) -> HashMap<VarId, Vec<usize>> {
    let mut sites: HashMap<VarId, Vec<usize>> = HashMap::new();
    for (scope, test, _) in tests {
        if let Some(var) = test.var {
            sites.entry(var).or_default().push(*scope);
        }
    }
    sites
}
