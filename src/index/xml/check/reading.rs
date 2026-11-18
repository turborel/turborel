//! \[EXTENSION\] RULES.md D106: a lookup's answer of a value, read as its
//! events come (`check.rs`), with its filter's [XQuery §3.2.2].
//!
//! An element at which the steps above a filter's end is an anchor: each
//! element open below it keeps, as bits, the filter's steps ending at it
//! and matched at or above it, from that anchor, as the lookup's own from
//! the item. A node a filter selects answers of its anchor, which holds
//! once one holds, and raises, as it closes, if none held and one raised.
//! A node selected whose comparisons hold, of an anchor not known to hold,
//! waits for its anchor, as an element selected and open does; the first
//! answer, by place, is kept until no node before it waits.

use super::{Compiled, Steps, bit, compared_by};
use crate::error::{Code, Result, SqlError};
use crate::index::xml::lookup::{Target, XmlFilter, XmlLookup};
use crate::xml::{Event, MAX_NESTING, QName};

/// An element open: the steps ending at it and matched at or above it;
/// if it is selected, its place in document order and where its text
/// begins; for each anchor open at or above it, its depth and the
/// filter's steps ending at it and matched at or above it from there; and
/// the anchors' depths whose filter selects it.
struct Open {
    ended: Steps,
    reached: Steps,
    selected: Option<(u64, usize)>,
    filtering: Vec<(usize, Steps, Steps)>,
    filtered: Vec<usize>,
    /// Where its text begins, if it is selected by the lookup or a filter.
    start: Option<usize>,
}

/// An anchor open: its depth; whether its filter holds, or the error one
/// of its nodes raised; and the first place of a node waiting for it.
struct Anchor {
    depth: usize,
    holds: bool,
    raised: Option<SqlError>,
    waiting: Option<u64>,
}

/// A filter compiled: its steps, the steps of the lookup's before its
/// anchor, and its anchor's distance above a node selected.
struct Filtering<'l> {
    filter: &'l XmlFilter,
    path: Compiled<'l>,
    anchor: Steps,
    above: usize,
}

/// The answer of a value, being read.
pub(super) struct Reading<'l> {
    lookup: &'l XmlLookup,
    path: Compiled<'l>,
    filter: Option<Filtering<'l>>,
    /// The steps, and the filter's steps, each name of the item passes.
    names: Vec<(Steps, Steps)>,
    open: Vec<Open>,
    anchors: Vec<Anchor>,
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
    /// The reading of `lookup`'s answer.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 of a path of no step, or of a filter not of an
    /// ancestor its last steps, child steps, reach; -900 of a path of
    /// more than `MAX_STEPS`.
    pub(super) fn of(lookup: &'l XmlLookup, filter: Option<&'l XmlFilter>) -> Result<Reading<'l>> {
        let filter = match filter {
            Some(filter) => {
                let anchor = lookup.anchored(filter)?;
                let path = Compiled::of(&filter.pattern.steps)?;
                Some(Filtering { filter, path, anchor: bit(u32::try_from(anchor).unwrap_or(u32::MAX)), above: filter.above })
            }
            None => None,
        };
        Ok(Reading {
            lookup,
            path: Compiled::of(&lookup.pattern.steps)?,
            filter,
            names: Vec::new(),
            open: Vec::new(),
            anchors: Vec::new(),
            pending: Vec::new(),
            text: String::new(),
            collecting: 0,
            at: 0,
            first: None,
        })
    }

    /// Takes `names`, the names of the item read.
    pub(super) fn named(&mut self, names: &[QName]) {
        let filtered = self.filter.as_ref().map(|f| f.path.names(names)).unwrap_or_default();
        self.names = self.path.names(names).into_iter().zip(filtered.into_iter().chain(std::iter::repeat(0))).collect();
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

    /// The steps, and the filter's, the name numbered `name` passes.
    fn passes(&self, name: Option<u32>) -> (Steps, Steps) {
        name.and_then(|n| usize::try_from(n).ok()).and_then(|n| self.names.get(n)).copied().unwrap_or((0, 0))
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
            None => (bit(0), bit(0), Vec::new()),
            Some(parent) => {
                let ended = self.path.child(passed, (parent.ended, parent.reached));
                (ended, parent.reached | ended, self.filtering_below(parent, filter_passed))
            }
        };
        if self.filter.as_ref().is_some_and(|f| ended & f.anchor != 0) {
            self.anchors.push(Anchor { depth, holds: false, raised: None, waiting: None });
            filtering.push((depth, bit(0), bit(0)));
        }
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
        self.open.push(Open { ended, reached, selected, filtering, filtered, start });
        Ok(())
    }

    /// The filter's steps of a child of name passing `passed`, of `parent`.
    fn filtering_below(&self, parent: &Open, passed: Steps) -> Vec<(usize, Steps, Steps)> {
        let Some(filter) = &self.filter else { return Vec::new() };
        let each = |&(anchor, ended, reached): &(usize, Steps, Steps)| {
            let below = filter.path.child(passed, (ended, reached));
            (anchor, below, reached | below)
        };
        parent.filtering.iter().map(each).collect()
    }

    /// The depths of the anchors whose filter selects a node of kind
    /// `kind` at which `filtering` end.
    fn filtered(&self, filtering: &[(usize, Steps, Steps)], kind: Target) -> Vec<usize> {
        let Some(filter) = self.filter.as_ref().filter(|f| f.filter.pattern.target == kind) else { return Vec::new() };
        filtering.iter().filter(|(_, ended, _)| filter.path.selects(*ended)).map(|&(anchor, _, _)| anchor).collect()
    }

    /// An attribute of the element open last, named `name`, of `value`.
    fn attribute(&mut self, name: u32, value: &str) {
        self.at = self.at.saturating_add(1);
        let Some(element) = self.open.last() else { return };
        let (passed, filter_passed) = self.passes(Some(name));
        let selected =
            self.lookup.pattern.target == Target::Attribute && self.path.selects(self.path.child(passed, (element.ended, element.reached)));
        let filtering = self.filtering_below(element, filter_passed);
        let parent = self.open.len().saturating_sub(1);
        for anchor in self.filtered(&filtering, Target::Attribute) {
            self.filter_answer(anchor, value);
        }
        if selected {
            self.target_answer((self.at, parent.saturating_add(1)), value);
        }
    }

    /// Closes the node open last, answering of it if it is selected, of
    /// its anchors if a filter selects it, and closing it as an anchor.
    pub(super) fn end(&mut self) {
        let Some(closed) = self.open.pop() else { return };
        let depth = self.open.len();
        let text = closed.start.and_then(|start| self.text.get(start..)).unwrap_or_default().to_owned();
        for &anchor in &closed.filtered {
            self.filter_answer(anchor, &text);
        }
        if let Some((at, _)) = closed.selected {
            self.pending.pop();
            self.target_answer((at, depth), &text);
        }
        if self.anchors.last().is_some_and(|a| a.depth == depth)
            && let Some(anchor) = self.anchors.pop()
        {
            // A node waiting for an anchor that does not hold answers as
            // its filter does: false, or the error it raised.
            if let Some((waiting, raised)) = anchor.waiting.zip(anchor.raised) {
                self.answer(waiting, Err(raised));
            }
        }
        if closed.start.is_some() {
            self.collecting = self.collecting.saturating_sub(1);
        }
        if self.collecting == 0 {
            self.text.clear();
        }
    }

    /// The answer of a node selected, at place `at` and `depth` deep, of
    /// value `text`: of its comparisons, then of its anchor's filter.
    fn target_answer(&mut self, (at, depth): (u64, usize), text: &str) {
        let compared = self.lookup.compared(text);
        let Some(above) = self.filter.as_ref().map(|f| f.above) else { return self.answer(at, compared) };
        if !matches!(compared, Ok(true)) {
            return self.answer(at, compared);
        }
        let anchor = depth.checked_sub(above).and_then(|d| self.anchors.iter_mut().find(|a| a.depth == d));
        let Some(anchor) = anchor else { return };
        if !anchor.holds {
            anchor.waiting = Some(anchor.waiting.map_or(at, |w| w.min(at)));
            return;
        }
        self.answer(at, Ok(true));
    }

    /// The answer of a node anchor `depth`'s filter selects, of value
    /// `text`: the anchor holds once one holds, answering of the node
    /// waiting for it; else keeps the first error raised.
    fn filter_answer(&mut self, depth: usize, text: &str) {
        let Some(compared) = self.filter.as_ref().map(|f| compared_by(&f.filter.comparisons, text)) else { return };
        let Some(anchor) = self.anchors.iter_mut().find(|a| a.depth == depth) else { return };
        let waiting = match compared {
            Ok(false) => None,
            Ok(true) => {
                anchor.holds = true;
                anchor.waiting.take()
            }
            Err(e) => {
                anchor.raised.get_or_insert(e);
                None
            }
        };
        if let Some(waiting) = waiting {
            self.answer(waiting, Ok(true));
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
    /// anchor.
    pub(super) fn answered(&mut self) -> Option<Result<bool>> {
        let (at, _) = self.first.as_ref()?;
        let waiting = self.anchors.iter().filter_map(|a| a.waiting);
        if self.pending.first().copied().into_iter().chain(waiting).any(|w| w < *at) {
            return None;
        }
        self.first.take().map(|(_, raised)| raised.map_or(Ok(true), Err))
    }
}
