//! Abstract syntax of `<window function>` [ISO/IEC 9075-2:2023 §6.10] and
//! `<window clause>` [§7.15].

use super::{GroupingColumn, ValueExpression, ValueOrder};

/// [ISO/IEC 9075-2:2023 §6.10] `<window function>` ::= `<window function
/// type>` OVER `<window name or specification>`
#[derive(Debug, Clone)]
pub struct WindowFunction {
    /// The `<window function type>`.
    pub kind: WindowKind,
    /// The window it is computed over.
    pub over: WindowOver,
    /// Where it begins in the statement: a character offset, for the
    /// positions of errors (`SqlError::or_at`).
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §6.10] `<window function type>`.
#[derive(Debug, Clone)]
pub enum WindowKind {
    /// ROW_NUMBER ( ).
    RowNumber,
    /// A `<rank function type>` ( ).
    Rank(RankKind),
    /// An `<aggregate function>`: a `<set function specification>`, or a
    /// call of a set function by name.
    Aggregate(Box<ValueExpression>),
}

/// [ISO/IEC 9075-2:2023 §6.10] `<rank function type>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RankKind {
    /// RANK.
    Rank,
    /// DENSE_RANK.
    DenseRank,
    /// PERCENT_RANK.
    PercentRank,
    /// CUME_DIST.
    CumeDist,
}

/// [ISO/IEC 9075-2:2023 §6.10] `<window name or specification>`.
#[derive(Debug, Clone)]
pub enum WindowOver {
    /// A `<window name>`, and where it is written.
    Name(String, usize),
    /// An `<in-line window specification>`.
    Specification(Box<WindowSpecification>),
}

/// [ISO/IEC 9075-2:2023 §7.15] `<window definition>` ::= `<new window
/// name>` AS `<window specification>`
#[derive(Debug, Clone)]
pub struct WindowDefinition {
    /// The `<new window name>`.
    pub name: String,
    /// Its `<window specification>`.
    pub specification: WindowSpecification,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §7.15] `<window specification details>`.
#[derive(Debug, Clone)]
pub struct WindowSpecification {
    /// The `<existing window name>`, if written, and where.
    pub existing: Option<(String, usize)>,
    /// The `<window partition column reference>`s of its `<window partition
    /// clause>`, none if it has none.
    pub partition: Vec<GroupingColumn>,
    /// The `<sort specification list>` of its `<window order clause>`,
    /// none if it has none.
    pub order: Vec<ValueOrder>,
    /// Its `<window frame clause>`, if written.
    pub frame: Option<Frame>,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §7.15] `<window frame clause>` ::= `<window frame
/// units>` `<window frame extent>` \[`<window frame exclusion>`\]
#[derive(Debug, Clone)]
pub struct Frame {
    /// ROWS, RANGE or GROUPS.
    pub units: FrameUnits,
    /// The `<window frame start>`, or `<window frame bound 1>`.
    pub start: FrameBound,
    /// The `<window frame bound 2>` of BETWEEN, if written; else CURRENT
    /// ROW \[SR11\].
    pub end: Option<FrameBound>,
    /// The `<window frame exclusion>`, EXCLUDE NO OTHERS if not written.
    pub exclusion: Exclusion,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §7.15] `<window frame units>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameUnits {
    /// ROWS: rows counted.
    Rows,
    /// RANGE: rows by their sort key's value.
    Range,
    /// GROUPS: peer groups counted.
    Groups,
}

/// [ISO/IEC 9075-2:2023 §7.15] `<window frame bound>`.
#[derive(Debug, Clone)]
pub enum FrameBound {
    /// UNBOUNDED PRECEDING.
    UnboundedPreceding,
    /// `<unsigned value specification>` PRECEDING.
    Preceding(Box<ValueExpression>),
    /// CURRENT ROW.
    CurrentRow,
    /// `<unsigned value specification>` FOLLOWING.
    Following(Box<ValueExpression>),
    /// UNBOUNDED FOLLOWING.
    UnboundedFollowing,
}

/// [ISO/IEC 9075-2:2023 §7.15] `<window frame exclusion>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Exclusion {
    /// EXCLUDE NO OTHERS, written or not.
    NoOthers,
    /// EXCLUDE CURRENT ROW.
    CurrentRow,
    /// EXCLUDE GROUP: the current row and its peers.
    Group,
    /// EXCLUDE TIES: the current row's peers, not itself.
    Ties,
}

impl WindowFunction {
    /// Calls `f` with each value expression it is made of: its aggregate's
    /// own parts, and those of its in-line window specification.
    pub(super) fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match &self.kind {
            WindowKind::RowNumber | WindowKind::Rank(_) => {}
            WindowKind::Aggregate(aggregate) => aggregate.for_each_child(f),
        }
        match &self.over {
            WindowOver::Name(..) => {}
            WindowOver::Specification(spec) => spec.for_each_child(f),
        }
    }
}

impl WindowSpecification {
    /// Calls `f` with each value expression it is made of: its sort keys and
    /// its frame's offsets. Its partition columns are column references
    /// alone.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        self.order.iter().for_each(|o| f(&o.key));
        if let Some(frame) = &self.frame {
            for bound in std::iter::once(&frame.start).chain(&frame.end) {
                match bound {
                    FrameBound::Preceding(v) | FrameBound::Following(v) => f(v),
                    FrameBound::UnboundedPreceding | FrameBound::CurrentRow | FrameBound::UnboundedFollowing => {}
                }
            }
        }
    }
}
