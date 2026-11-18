//! \[EXTENSION\] RULES.md D106: a query's answer of a value, read as its
//! events come (`check.rs`), with its filters' [XQuery §3.2.2].
//!
//! An element at which the lookup's steps up to a filter's step end is an
//! anchor of that filter: each element open below it keeps, as bits, the
//! filter's steps ending at it and matched at or above it, from that
//! anchor, as the lookup's own from the item. A node a filter selects
//! answers of its anchor, which holds once one holds, and, as it closes,
//! does not, raising if one raised.
//!
//! A node selected whose comparisons hold answers once each of its
//! filters' anchors, fixed by its place (`Anchored`), is known: true once
//! all hold; false once one closes not holding of no error; else, all
//! known, the error one raised. Until then it waits, as an element
//! selected and open does; the first answer, by place, is kept until no
//! node before it waits. Nodes waiting for the same anchors, alike in
//! whether one raised, answer alike, so only the first is kept. A query's
//! relation of an element to a node selected is read too (`relating.rs`),
//! a node waiting for an element following it as for an anchor.

mod relating;

use super::{Compiled, Steps, bit, compared_by};
use crate::error::{Code, Result, SqlError};
use crate::index::xml::lookup::{Anchored, Target, XmlFilter, XmlLookup, anchors_of};
use crate::index::xml::related::XmlRelated;
use crate::xml::{Event, MAX_NESTING, QName};
use relating::{Of, Related, Relating};

/// A filter's steps from an anchor, of an element open: the anchor's
/// number, and the steps ending at the element and matched at or above it.
#[derive(Clone, Copy)]
struct Filtering {
    anchor: u64,
    ended: Steps,
    reached: Steps,
}

/// An element open: the steps ending at it and matched at or above it;
/// if it is selected, its place in document order and where its text
/// begins; of each filter, its steps from each anchor open at or above
/// it; the anchors whose filter selects it; where its text begins, if it
/// is selected by the lookup or a filter; and how many anchors it is.
struct Open {
    ended: Steps,
    reached: Steps,
    selected: Option<(u64, usize)>,
    filtering: Vec<Vec<Filtering>>,
    filtered: Vec<u64>,
    start: Option<usize>,
    anchors: usize,
}

/// An anchor open: its number, its filter, its depth; whether its filter
/// holds, or the error one of its nodes raised.
struct Anchor {
    id: u64,
    filter: usize,
    depth: usize,
    holds: bool,
    raised: Option<SqlError>,
}

/// A node waiting for its anchors: its place, the anchors not yet known,
/// and the error one known raised.
struct Waiting {
    at: u64,
    pending: Vec<u64>,
    raised: Option<SqlError>,
}

/// A filter compiled: it, its steps, the bit of the lookup's step its
/// anchor is of, and where its anchor is of a node selected.
struct Compiling<'l> {
    filter: &'l XmlFilter,
    path: Compiled<'l>,
    step: Steps,
    anchored: Anchored,
}

/// The answer of a value, being read.
pub(super) struct Reading<'l> {
    lookup: &'l XmlLookup,
    /// The relation of an element to a node selected, if any.
    relating: Option<Relating<'l>>,
    path: Compiled<'l>,
    filters: Vec<Compiling<'l>>,
    /// The steps each name of the item passes, and each filter's.
    names: Vec<(Steps, Vec<Steps>)>,
    open: Vec<Open>,
    anchors: Vec<Anchor>,
    /// The number the next anchor takes.
    next_anchor: u64,
    waiting: Vec<Waiting>,
    /// The places of the elements selected and open, outermost first.
    pending: Vec<u64>,
    /// The text since the outermost element selected, by the lookup or a
    /// filter, and open began.
    text: String,
    /// The elements selected and open.
    collecting: usize,
    /// The place of the node met last.
    at: u64,
    /// The first answer, by place: true, or the error raised.
    first: Option<(u64, Option<SqlError>)>,
}

impl<'l> Reading<'l> {
    /// The reading of the answer of `lookup`, `filters` and `related`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 of a path of no step, or of a filter whose anchor is
    /// not fixed (`XmlLookup::anchored`); -900 of a path of more than
    /// `MAX_STEPS` steps, or of more filters than `MAX_FILTERS`.
    pub(super) fn of(lookup: &'l XmlLookup, filters: &'l [XmlFilter], related: Option<&'l XmlRelated>) -> Result<Reading<'l>> {
        let filters = filters
            .iter()
            .zip(anchors_of(lookup, filters)?)
            .map(|(filter, anchored)| {
                let step = bit(u32::try_from(filter.at).unwrap_or(u32::MAX));
                Ok(Compiling { filter, path: Compiled::of(&filter.pattern.steps)?, step, anchored })
            })
            .collect::<Result<_>>()?;
        Ok(Reading {
            lookup,
            relating: related.map(Relating::of),
            path: Compiled::of(&lookup.pattern.steps)?,
            filters,
            names: Vec::new(),
            open: Vec::new(),
            anchors: Vec::new(),
            next_anchor: 0,
            waiting: Vec::new(),
            pending: Vec::new(),
            text: String::new(),
            collecting: 0,
            at: 0,
            first: None,
        })
    }

    /// Takes `names`, the names of the item read.
    pub(super) fn named(&mut self, names: &[QName]) {
        let filtered: Vec<Vec<Steps>> = self.filters.iter().map(|f| f.path.names(names)).collect();
        let each = |n: usize| filtered.iter().map(|f| f.get(n).copied().unwrap_or(0)).collect();
        self.names = self.path.names(names).into_iter().enumerate().map(|(n, own)| (own, each(n))).collect();
        if let Some(relating) = self.relating.as_mut() {
            relating.named(names);
        }
    }

    /// Takes `event`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if elements nest past `MAX_NESTING`.
    pub(super) fn event(&mut self, event: Event<'_>) -> Result<()> {
        match event {
            Event::Node | Event::Atomic | Event::Other | Event::Attribute { .. } => {}
            Event::Document => self.push(None)?,
            Event::Element { name, attributes } => {
                self.push(Some(name))?;
                for (attribute, value) in attributes {
                    self.attribute(attribute, value);
                }
            }
            Event::Text(text) => {
                if self.collecting > 0 {
                    self.text.push_str(text);
                }
            }
            Event::End => self.end(),
        }
        Ok(())
    }

    /// The steps, and each filter's, the name numbered `name` passes.
    fn passes(&self, name: Option<u32>) -> (Steps, Vec<Steps>) {
        let passed = name.and_then(|n| usize::try_from(n).ok()).and_then(|n| self.names.get(n));
        passed.cloned().unwrap_or_else(|| (0, vec![0; self.filters.len()]))
    }

    /// Opens a document node, or an element named `name`: its item's root,
    /// at which no step but none ends, if nothing is open.
    fn push(&mut self, name: Option<u32>) -> Result<()> {
        if self.open.len() > MAX_NESTING {
            return Err(SqlError::new(
                Code::Corrupt,
                format!("[DESIGN] a stored XML value nests deeper than {MAX_NESTING} (RULES.md D106)"),
            ));
        }
        let depth = self.open.len();
        let (passed, filter_passed) = self.passes(name);
        let (ended, reached, mut filtering) = match self.open.last() {
            None => (bit(0), bit(0), vec![Vec::new(); self.filters.len()]),
            Some(parent) => {
                let ended = self.path.child(passed, (parent.ended, parent.reached));
                (ended, parent.reached | ended, self.filtering_below(parent, &filter_passed))
            }
        };
        let anchors = self.anchor(ended, depth, &mut filtering);
        // The item's root is no node below it, so takes no place.
        if depth > 0 {
            self.at = self.at.saturating_add(1);
        }
        let selected = (depth > 0 && self.lookup.pattern.target == Target::Element && self.path.selects(ended)).then_some(self.at);
        let filtered = self.filtered(&filtering, Target::Element);
        let start = (selected.is_some() || !filtered.is_empty()).then_some(self.text.len());
        if let Some(at) = selected {
            self.pending.push(at);
        }
        if start.is_some() {
            self.collecting = self.collecting.saturating_add(1);
        }
        let selected = selected.zip(start);
        self.open.push(Open { ended, reached, selected, filtering, filtered, start, anchors });
        let held = self.relating.as_mut().map(|r| r.push(name)).unwrap_or_default();
        for id in held {
            self.known(id, None);
        }
        Ok(())
    }

    /// Opens an anchor of each filter whose step ends at an element `depth`
    /// deep at which `ended` end, its steps from there among `filtering`;
    /// how many.
    fn anchor(&mut self, ended: Steps, depth: usize, filtering: &mut [Vec<Filtering>]) -> usize {
        let mut made = 0_usize;
        for ((filter, compiling), steps) in self.filters.iter().enumerate().zip(filtering.iter_mut()) {
            if ended & compiling.step == 0 {
                continue;
            }
            let id = self.next_anchor;
            self.next_anchor = self.next_anchor.saturating_add(1);
            self.anchors.push(Anchor { id, filter, depth, holds: false, raised: None });
            steps.push(Filtering { anchor: id, ended: bit(0), reached: bit(0) });
            made = made.saturating_add(1);
        }
        made
    }

    /// Each filter's steps of a child of `parent` whose name passes
    /// `passed`, each filter's.
    fn filtering_below(&self, parent: &Open, passed: &[Steps]) -> Vec<Vec<Filtering>> {
        let below = |(compiling, each): (&Compiling<'_>, &Vec<Filtering>), name: Steps| -> Vec<Filtering> {
            let step = |f: &Filtering| {
                let ended = compiling.path.child(name, (f.ended, f.reached));
                Filtering { anchor: f.anchor, ended, reached: f.reached | ended }
            };
            each.iter().map(step).collect()
        };
        self.filters.iter().zip(&parent.filtering).zip(passed.iter().copied()).map(|(of, p)| below(of, p)).collect()
    }

    /// The anchors whose filter selects a node of kind `kind` at which
    /// `filtering` end.
    fn filtered(&self, filtering: &[Vec<Filtering>], kind: Target) -> Vec<u64> {
        let of = |(compiling, each): (&Compiling<'_>, &Vec<Filtering>)| {
            let selects = compiling.filter.pattern.target == kind;
            each.iter().filter(move |f| selects && compiling.path.selects(f.ended)).map(|f| f.anchor).collect::<Vec<_>>()
        };
        self.filters.iter().zip(filtering).flat_map(of).collect()
    }

    /// An attribute of the element open last, named `name`, of `value`.
    fn attribute(&mut self, name: u32, value: &str) {
        self.at = self.at.saturating_add(1);
        let Some(element) = self.open.last() else { return };
        let (passed, filter_passed) = self.passes(Some(name));
        let selected =
            self.lookup.pattern.target == Target::Attribute && self.path.selects(self.path.child(passed, (element.ended, element.reached)));
        let filtering = self.filtering_below(element, &filter_passed);
        let parent = self.open.len().saturating_sub(1);
        for anchor in self.filtered(&filtering, Target::Attribute) {
            self.filter_answer(anchor, value);
        }
        if selected {
            self.target_answer((self.at, parent.saturating_add(1)), value, Of::Attribute);
        }
    }

    /// Closes the node open last, answering of it if it is selected, of
    /// its anchors if a filter selects it, and closing its anchors.
    pub(super) fn end(&mut self) {
        let Some(closed) = self.open.pop() else { return };
        let opened = self.relating.as_mut().and_then(Relating::pop);
        let depth = self.open.len();
        let text = closed.start.and_then(|start| self.text.get(start..)).unwrap_or_default().to_owned();
        for &anchor in &closed.filtered {
            self.filter_answer(anchor, &text);
        }
        if let Some((at, _)) = closed.selected {
            self.pending.pop();
            // An element is open in the relation as in the reading, so of a
            // relation it has its own.
            if let Some(opened) = opened.as_ref() {
                self.target_answer((at, depth), &text, Of::Element(opened));
            } else {
                self.target_answer((at, depth), &text, Of::Attribute);
            }
        }
        let ended = self.relating.as_mut().zip(opened).map(|(r, o)| r.closed(&o)).unwrap_or_default();
        for id in ended {
            self.waiting.retain(|w| !w.pending.contains(&id));
        }
        for _ in 0..closed.anchors {
            if let Some(anchor) = self.anchors.pop() {
                self.close(&anchor);
            }
        }
        if closed.start.is_some() {
            self.collecting = self.collecting.saturating_sub(1);
        }
        if self.collecting == 0 {
            self.text.clear();
        }
    }

    /// The answer of a node selected, `of` an element or an attribute, at
    /// place `at` and `depth` deep, of value `text`: of its comparisons,
    /// then of its anchors' filters and its relation.
    fn target_answer(&mut self, (at, depth): (u64, usize), text: &str, of: Of<'_>) {
        let compared = self.lookup.compared(text);
        if !matches!(compared, Ok(true)) {
            return self.answer(at, compared);
        }
        let mut pending = Vec::new();
        for (filter, compiling) in self.filters.iter().enumerate() {
            let below = match compiling.anchored {
                Anchored::Above(n) => depth.checked_sub(n),
                Anchored::Below(n) => Some(n),
            };
            // An anchor never opened, as of an attribute, holds of nothing.
            let Some(anchor) = self.anchors.iter().find(|a| a.filter == filter && Some(a.depth) == below) else { return };
            if !anchor.holds {
                pending.push(anchor.id);
            }
        }
        match self.relating.as_mut().map(|r| r.related(of, &mut self.next_anchor)) {
            Some(Related::No) => return,
            Some(Related::Waits(id)) => pending.push(id),
            Some(Related::Yes) | None => {}
        }
        if pending.is_empty() {
            return self.answer(at, Ok(true));
        }
        pending.sort_unstable();
        match self.waiting.iter_mut().find(|w| w.pending == pending && w.raised.is_none()) {
            Some(alike) => alike.at = alike.at.min(at),
            None => self.waiting.push(Waiting { at, pending, raised: None }),
        }
    }

    /// The answer of a node anchor `id`'s filter selects, of value `text`:
    /// the anchor holds once one holds, answering of the nodes waiting for
    /// it; else keeps the first error raised.
    fn filter_answer(&mut self, id: u64, text: &str) {
        let Some(anchor) = self.anchors.iter_mut().find(|a| a.id == id) else { return };
        let Some(compiling) = self.filters.get(anchor.filter) else { return };
        match compared_by(&compiling.filter.comparisons, text) {
            Ok(false) => {}
            Ok(true) => {
                let first = !anchor.holds;
                anchor.holds = true;
                if first {
                    self.known(id, None);
                }
            }
            Err(e) => {
                anchor.raised.get_or_insert(e);
            }
        }
    }

    /// Anchor `anchor` closed: of the nodes waiting for it, none holds if
    /// it does not hold and raised none; each else knows the error.
    fn close(&mut self, anchor: &Anchor) {
        if anchor.holds {
            return;
        }
        match &anchor.raised {
            None => self.waiting.retain(|w| !w.pending.contains(&anchor.id)),
            Some(e) => self.known(anchor.id, Some(e)),
        }
    }

    /// Anchor `id` known: holding, or of the error `raised`. Each node
    /// waiting for it waits for it no longer, answering once it waits for
    /// none: true, or the error one of its anchors raised.
    fn known(&mut self, id: u64, raised: Option<&SqlError>) {
        let mut answered = Vec::new();
        for w in &mut self.waiting {
            if !w.pending.contains(&id) {
                continue;
            }
            w.pending.retain(|&p| p != id);
            if let Some(e) = raised {
                w.raised.get_or_insert_with(|| e.clone());
            }
            if w.pending.is_empty() {
                answered.push((w.at, w.raised.take()));
            }
        }
        self.waiting.retain(|w| !w.pending.is_empty());
        for (at, error) in answered {
            self.answer(at, error.map_or(Ok(true), Err));
        }
    }

    /// The answer `answer` of the node at place `at`: kept if it holds or
    /// raises, and is the first so far.
    fn answer(&mut self, at: u64, answer: Result<bool>) {
        let kept = match answer {
            Ok(false) => return,
            Ok(true) => None,
            Err(e) => Some(e),
        };
        if self.first.as_ref().is_none_or(|(first, _)| at < *first) {
            self.first = Some((at, kept));
        }
    }

    /// The answer, once no node before the first that answers may still
    /// answer: an element selected and open, or a node waiting for its
    /// anchors.
    pub(super) fn answered(&mut self) -> Option<Result<bool>> {
        let (at, _) = self.first.as_ref()?;
        let waiting = self.waiting.iter().map(|w| w.at);
        if self.pending.first().copied().into_iter().chain(waiting).any(|w| w < *at) {
            return None;
        }
        self.first.take().map(|(_, raised)| raised.map_or(Ok(true), Err))
    }
}
