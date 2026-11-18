//! \[EXTENSION\] RULES.md D106: what an XML index is asked, by the XQuery
//! evaluator or the planner: the rows whose value may hold a node of a
//! simple path [XQuery §3.2], optionally of a value a general comparison
//! with a literal selects [XQuery §3.5.2] (`Index::xml_rows`), and what
//! finding them costs (`Index::xml_cost`).
//!
//! A path is a chain of steps from each item of a row's value, the
//! context node of its first step: each a child step, `/name`, or a
//! descendant step, `//name`, short for `/descendant-or-self::node()/
//! child::name` [XQuery §3.2.4]; the last selects an element or, along
//! the attribute axis, an attribute. A name is an expanded QName, or any
//! name (`*`) [XQuery §3.2.1.2].

use super::dictionary::{Dictionary, tokens_of};
use super::keys::{ATTRIBUTE, ELEMENT, NUMBER, STRING, UNORDERED, after, number_value, put_ordinal, string_value};
use crate::disk::pager::DEFAULT_PAGE_SIZE;
use crate::error::{Code, Result, SqlError};
use crate::storage::RowId;
use crate::value::CompOp;
use crate::xml::QName;

/// \[DESIGN\] How many steps a path a lookup is given has at most: deeper
/// is SQLCODE -900 (RULES.md D18). Each step costs the match of a path a
/// pass over the path's names, so the bound bounds a lookup's work; the
/// value is no measurement, but past any path a query names.
pub const MAX_STEPS: usize = 64;

/// \[DESIGN\] How many filters a query has at most: more is SQLCODE -900
/// (RULES.md D18). Each is read of each of its anchors, so the bound
/// bounds a check's work; the value is no measurement, but past the
/// predicates a query writes.
pub const MAX_FILTERS: usize = 16;

/// How a step reaches its nodes from the step before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// `/name`: a child of the node before, or of the item for the first
    /// step; for the last step, an attribute's, its attribute.
    Child,
    /// `//name`: a child of the node before or of any of its descendants.
    Descendant,
}

/// A step of a path: its axis and its name test.
#[derive(Debug, Clone)]
pub struct Step {
    /// How it reaches its nodes.
    pub axis: Axis,
    /// The expanded QName its nodes have, its prefix ignored; none for
    /// any name.
    pub name: Option<QName>,
}

/// What the last step of a path selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Elements.
    Element,
    /// Attributes.
    Attribute,
}

/// A path of steps from each item of a value.
#[derive(Debug, Clone)]
pub struct PathPattern {
    /// Its steps, from the item down: at least one.
    pub steps: Vec<Step>,
    /// What its last step selects.
    pub target: Target,
}

/// A literal a node's value is compared with.
#[derive(Debug, Clone)]
pub enum Literal {
    /// An xs:string: the node's `xdt:untypedAtomic` value is compared as
    /// a string, by code points [XQuery §3.5.2] [XQueryFO §7.3.1].
    String(String),
    /// An `xdt:untypedAtomic`, compared as a string as well.
    Untyped(String),
    /// A number, promoted to xs:double: the node's value is cast to
    /// xs:double [XQuery §3.5.2] [XQueryFO §17.1.1].
    Number(f64),
}

/// A general comparison of a node's value with a literal.
#[derive(Debug, Clone)]
pub struct Comparison {
    /// `=`, `!=`, `<`, `<=`, `>` or `>=`.
    pub op: CompOp,
    /// The literal on its right.
    pub literal: Literal,
}

/// \[EXTENSION\] RULES.md D106: a lookup of an XML index.
#[derive(Debug, Clone)]
pub struct XmlLookup {
    /// The nodes it selects.
    pub pattern: PathPattern,
    /// Comparisons each holding of one node selected: none for the path
    /// alone.
    pub comparisons: Vec<Comparison>,
    /// The one row asked of, if any: a property's.
    pub row: Option<RowId>,
}

/// \[EXTENSION\] RULES.md D106: a lookup and filters of the nodes of its
/// steps: what an index answers of a row from its entries alone
/// (`Index::xml_answers`), and a value's own check answers
/// (`XmlQuery::matches`). It holds of a value of which its lookup selects
/// a node whose every filter holds of its anchor. Of an index that only
/// finds rows, the rows every lookup of it gives (`XmlQuery::lookups`)
/// are a superset of its rows.
#[derive(Debug, Clone)]
pub struct XmlQuery {
    /// The lookup.
    pub lookup: XmlLookup,
    /// Predicates of the nodes of the lookup's steps, each of the node of
    /// one, its anchor: none for the lookup alone.
    pub filters: Vec<XmlFilter>,
}

/// \[EXTENSION\] RULES.md D106 [XQuery §3.2.2]: a predicate of the node a
/// step of a lookup selects, its anchor, as `/BOOK[@ISBN = "1"]/SECTION`
/// holds of a SECTION whose parent BOOK has an ISBN of "1" [Pal et al.
/// §3.1.2]: it holds of the anchor if a node its path selects from the
/// anchor has a value its comparisons hold of; if none does and one
/// raises, it raises. The anchor of each node selected is fixed
/// (`Anchored`).
#[derive(Debug, Clone)]
pub struct XmlFilter {
    /// The step of the lookup whose node is the anchor: 0 the item, 1 the
    /// node of its first step, its last the node selected.
    pub at: usize,
    /// The nodes it selects, from the anchor.
    pub pattern: PathPattern,
    /// Comparisons each holding of one node it selects.
    pub comparisons: Vec<Comparison>,
}

/// \[EXTENSION\] RULES.md D106 [Pal et al. §2.2]: where a filter's anchor
/// is of a node its lookup selects, one node of its ancestors or itself:
/// a filter whose step is followed by child steps alone, or reached by
/// child steps alone, has one, its ORDPATH a prefix of the node's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchored {
    /// As many steps above the node: the lookup's steps after the
    /// filter's are child steps.
    Above(usize),
    /// As many steps below the item: the lookup's steps to the filter's
    /// are child steps.
    Below(usize),
}

/// \[EXTENSION\] RULES.md D106: what an index knows of a row's value from
/// its entries alone (`Index::xml_answers`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlAnswer {
    /// The lookup holds of it.
    Holds,
    /// It may hold, or raise: its value is checked (`XmlLookup::matches`).
    Check,
}

/// \[EXTENSION\] The rows an index answers a lookup of, each once, as they
/// are read; a row not given does not hold.
pub type XmlAnswers = Box<dyn Iterator<Item = Result<(RowId, XmlAnswer)>> + Send>;

/// \[EXTENSION\] RULES.md D106, D35: what a lookup costs, as an index
/// counts it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XmlCost {
    /// The entries it reads, before rows are given once, if counted
    /// within a bound; none past it.
    pub entries: Option<usize>,
    /// The pages it reads: its seeks, each its tree's height, and the
    /// pages of the entries it reads, if known.
    pub pages: Option<f64>,
}

impl XmlCost {
    /// \[DESIGN\] What checking a value of `octets` octets stored costs
    /// (`XmlLookup::matches`), to weigh against an index's answer: one pass
    /// over its stored form, of its pages of the default size.
    #[must_use]
    pub fn of_check(octets: u64) -> XmlCost {
        let pages = octets.div_ceil(u64::from(DEFAULT_PAGE_SIZE)).max(1);
        XmlCost { entries: None, pages: Some(u32::try_from(pages).map_or(f64::from(u32::MAX), f64::from)) }
    }
}

impl XmlLookup {
    /// Where `filter`'s anchor is of a node it selects.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the filter is of no step of it, or of one whose
    /// node is not fixed of a node it selects: of a descendant step before
    /// it and after it (RULES.md D106).
    pub fn anchored(&self, filter: &XmlFilter) -> Result<Anchored> {
        let steps = &self.pattern.steps;
        let child = |some: &[Step]| some.iter().all(|s| s.axis == Axis::Child);
        let (to, after) = steps
            .split_at_checked(filter.at)
            .ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] an XML query's filter is of a step past its lookup's (RULES.md D106)"))?;
        if child(after) {
            return Ok(Anchored::Above(after.len()));
        }
        if child(to) {
            return Ok(Anchored::Below(to.len()));
        }
        Err(SqlError::new(
            Code::Api,
            "[DESIGN] an XML query's filter is of a step whose node is not fixed of a node it selects, a descendant step before and after it (RULES.md D106)",
        ))
    }
}

/// Where each of `filters`' anchor is of a node `lookup` selects.
///
/// # Errors
///
/// As `XmlLookup::anchored`; SQLCODE -900 of more than `MAX_FILTERS`
/// filters.
pub(super) fn anchors_of(lookup: &XmlLookup, filters: &[XmlFilter]) -> Result<Vec<Anchored>> {
    if filters.len() > MAX_FILTERS {
        return Err(SqlError::new(
            Code::Unsupported,
            format!("[DESIGN] an XML query takes at most {MAX_FILTERS} filters (RULES.md D18, D106)"),
        ));
    }
    filters.iter().map(|f| lookup.anchored(f)).collect()
}

impl XmlQuery {
    /// The lookup alone, of no filter.
    #[must_use]
    pub fn of(lookup: XmlLookup) -> XmlQuery {
        XmlQuery { lookup, filters: Vec::new() }
    }

    /// Where each filter's anchor is of a node its lookup selects.
    ///
    /// # Errors
    ///
    /// As `XmlLookup::anchored`; SQLCODE -900 of more than `MAX_FILTERS`
    /// filters.
    pub fn anchors(&self) -> Result<Vec<Anchored>> {
        anchors_of(&self.lookup, &self.filters)
    }

    /// Its lookup's rows hold every row it holds of, and so do each
    /// filter's: the lookup's steps to the filter's anchor, then the
    /// filter's, with its comparisons [Pal et al. §3.3]. Its lookup, then
    /// each filter's, of the row its lookup asks of.
    #[must_use]
    pub fn lookups(&self) -> Vec<XmlLookup> {
        let each = self.filters.iter().map(|filter| {
            let to = self.lookup.pattern.steps.get(..filter.at).unwrap_or(&self.lookup.pattern.steps);
            let steps = to.iter().chain(&filter.pattern.steps).cloned().collect();
            let pattern = PathPattern { steps, target: filter.pattern.target };
            XmlLookup { pattern, comparisons: filter.comparisons.clone(), row: self.lookup.row }
        });
        std::iter::once(self.lookup.clone()).chain(each).collect()
    }
}

/// A path resolved by an index's dictionary: its kind, its steps' axes and
/// tokens, the dictionary's prefix of the paths it may match, and whether
/// only the path that prefix makes matches.
pub(super) struct Resolved {
    kind: u8,
    steps: Vec<(Axis, Option<u64>)>,
    pub(super) prefix: Vec<u8>,
    pub(super) exact: bool,
}

impl PathPattern {
    /// The path resolved by `dictionary`: none if a name it tests has no
    /// token, so no node has it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 of a path of no step; -900 of one past `MAX_STEPS`;
    /// the dictionary's.
    pub(super) fn resolve(&self, dictionary: &Dictionary<'_>) -> Result<Option<Resolved>> {
        if self.steps.is_empty() {
            return Err(SqlError::new(Code::Api, "[DESIGN] an XML index's lookup is given a path of no step (RULES.md D106)"));
        }
        if self.steps.len() > MAX_STEPS {
            return Err(SqlError::new(
                Code::Unsupported,
                format!("[DESIGN] an XML index's lookup takes at most {MAX_STEPS} steps (RULES.md D18, D106)"),
            ));
        }
        let mut steps = Vec::with_capacity(self.steps.len());
        for step in &self.steps {
            let token = match &step.name {
                Some(name) => match dictionary.name(name, false)? {
                    Some(token) => Some(token),
                    None => return Ok(None),
                },
                None => None,
            };
            steps.push((step.axis, token));
        }
        let kind = match self.target {
            Target::Element => ELEMENT,
            Target::Attribute => ATTRIBUTE,
        };
        let (prefix, exact) = prefix_of(kind, &steps);
        Ok(Some(Resolved { kind, steps, prefix, exact }))
    }
}

/// The dictionary's prefix of the paths `steps` may match, the kind then
/// the tokens of the steps from the last up while each is named, to the
/// first descendant step's; and whether it is a whole path, every step a
/// named child step.
fn prefix_of(kind: u8, steps: &[(Axis, Option<u64>)]) -> (Vec<u8>, bool) {
    let mut prefix = vec![kind];
    for (axis, token) in steps.iter().rev() {
        let Some(token) = token else { return (prefix, false) };
        put_ordinal(&mut prefix, *token);
        if *axis == Axis::Descendant {
            return (prefix, false);
        }
    }
    (prefix, true)
}

impl Resolved {
    /// Whether the path `path`, `[kind][tokens]`, matches: of its kind, its
    /// names those its steps test, in order, each child step's directly
    /// below the step before's.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the path is malformed.
    pub(super) fn matches(&self, path: &[u8]) -> Result<bool> {
        let (kind, mut tokens) = tokens_of(path)?;
        tokens.reverse();
        Ok(self.selects(kind, &tokens))
    }

    /// Whether a node of kind `kind`, of `names` from below its item, or
    /// its anchor, down to it, matches.
    pub(super) fn selects(&self, kind: u8, names: &[u64]) -> bool {
        kind == self.kind && steps_match(&self.steps, names)
    }
}

/// Whether `steps` select the last of `names`, from the item down: by the
/// steps matched so far ending at each name, one step at a time, so a path
/// costs its steps times its names, recursing nowhere.
pub(super) fn steps_match<S: PartialEq<T>, T>(steps: &[(Axis, Option<S>)], names: &[T]) -> bool {
    // `ended[j]`: the steps so far match, the last at name `j - 1`; at
    // first, none taken, at the item.
    let mut ended = vec![false; names.len().saturating_add(1)];
    if let Some(start) = ended.first_mut() {
        *start = true;
    }
    for (axis, test) in steps {
        let mut next = vec![false; ended.len()];
        let mut any_before = false;
        for (j, name) in names.iter().enumerate() {
            let before = ended.get(j).copied().unwrap_or(false);
            any_before |= before;
            let reached = match axis {
                Axis::Child => before,
                Axis::Descendant => any_before,
            };
            if let Some(slot) = next.get_mut(j.saturating_add(1)) {
                *slot = reached && test.as_ref().is_none_or(|t| t == name);
            }
        }
        ended = next;
    }
    ended.last().copied().unwrap_or(false)
}

/// \[DESIGN\] The range of value keys a node's value is in, if the
/// comparisons hold of it: from `low`, below `high`. Keys order as values
/// do (`keys.rs`), a string's cut by its first octets, so a range holds
/// every value that may hold, and a lookup's rows are checked again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ValueRange {
    pub(super) low: Vec<u8>,
    pub(super) high: Vec<u8>,
}

impl ValueRange {
    /// Whether `value`, a value key, is in it.
    pub(super) fn holds(&self, value: &[u8]) -> bool {
        self.low.as_slice() <= value && value < self.high.as_slice()
    }
}

/// The key after every key beginning with `prefix`, which begins with a
/// class of value, below FF.
fn past(prefix: &[u8]) -> Vec<u8> {
    match after(prefix) {
        std::ops::Bound::Excluded(next) | std::ops::Bound::Included(next) => next,
        std::ops::Bound::Unbounded => vec![u8::MAX],
    }
}

/// The ranges of value keys `comparisons` select; none if they constrain
/// no value. Comparisons of the first's kind, string or number, are
/// taken together; another's are left to the check of each row. A number
/// compared also selects the unordered values, whose cast fails or which
/// were cut, so no row whose comparison would raise err:FORG0001 is
/// skipped (RULES.md D15).
pub(super) fn value_ranges(comparisons: &[Comparison], prefix: usize) -> Option<Vec<ValueRange>> {
    let ranged: Vec<(u8, ValueRange)> = comparisons.iter().filter_map(|c| range_of(c, prefix)).collect();
    let (class, _) = ranged.first()?;
    let class = *class;
    let mut range = ValueRange { low: vec![class], high: past(&[class]) };
    for (_, each) in ranged.iter().filter(|(c, _)| *c == class) {
        range.low = range.low.max(each.low.clone());
        range.high = range.high.min(each.high.clone());
    }
    let mut selected = Vec::new();
    if range.low < range.high {
        selected.push(range);
    }
    if class == NUMBER {
        selected.push(ValueRange { low: vec![UNORDERED], high: past(&[UNORDERED]) });
    }
    Some(selected)
}

/// The class of value, and the range of its keys, `comparison` selects;
/// none for `!=`, which a range cannot hold.
fn range_of(comparison: &Comparison, prefix: usize) -> Option<(u8, ValueRange)> {
    let (class, key) = match &comparison.literal {
        Literal::String(s) | Literal::Untyped(s) => (STRING, string_value(s.as_bytes(), prefix)),
        // NaN equals, and is less or greater than, nothing [XQueryFO §6.3].
        Literal::Number(n) => {
            if n.is_nan() {
                return Some((NUMBER, ValueRange { low: vec![NUMBER], high: vec![NUMBER] }));
            }
            (NUMBER, number_value(*n))
        }
    };
    let (start, end) = (vec![class], past(&[class]));
    // A number's key is exact; a string's of a cut, so its ties are kept,
    // each checked.
    let (above, below) = if class == NUMBER { (past(&key), key.clone()) } else { (key.clone(), past(&key)) };
    let range = match comparison.op {
        CompOp::Eq => ValueRange { low: key.clone(), high: past(&key) },
        CompOp::Ne => return None,
        CompOp::Lt => ValueRange { low: start, high: below },
        CompOp::Le => ValueRange { low: start, high: past(&key) },
        CompOp::Gt => ValueRange { low: above, high: end },
        CompOp::Ge => ValueRange { low: key, high: end },
    };
    Some((class, range))
}

// Unit tests: paths matched step by step, prefixes, ranges (RULES.md
// R-TEST-1, R-TEST-2, R-TEST-8).
#[cfg(test)]
mod tests;
