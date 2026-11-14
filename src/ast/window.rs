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
    /// `<ntile function>` ::= NTILE ( `<number of tiles>` ).
    Ntile(Box<ValueExpression>),
    /// A `<lead or lag function>`.
    Offset(Box<OffsetFunction>),
    /// A `<first or last value function>` or an `<nth value function>`.
    Value(Box<ValueFunction>),
}

/// [ISO/IEC 9075-2:2023 §6.10] `<lead or lag function>` ::= `<lead or lag>`
/// ( `<lead or lag extent>` \[, `<offset>` \[, `<default expression>`\]\]
/// ) \[`<window function null treatment>`\]
#[derive(Debug, Clone)]
pub struct OffsetFunction {
    /// LEAD rather than LAG.
    pub lead: bool,
    /// The `<lead or lag extent>`.
    pub extent: ValueExpression,
    /// The `<offset>`, an `<unsigned integer>`, if written; 1 if not
    /// [§9.23 SR5j ii].
    pub offset: Option<u32>,
    /// The `<default expression>`, if written.
    pub default: Option<ValueExpression>,
    /// IGNORE NULLS rather than RESPECT NULLS, written or implied.
    pub ignore_nulls: bool,
}

/// [ISO/IEC 9075-2:2023 §6.10] `<first or last value function>` ::= `<first
/// or last value>` ( `<value expression>` ) \[`<window function null
/// treatment>`\], or `<nth value function>` ::= NTH_VALUE ( `<value
/// expression>` , `<nth row>` ) \[`<from first or last>`\] \[`<window
/// function null treatment>`\]
#[derive(Debug, Clone)]
pub struct ValueFunction {
    /// Which row of the frame it takes the value of.
    pub which: Which,
    /// The `<value expression>`.
    pub arg: ValueExpression,
    /// IGNORE NULLS rather than RESPECT NULLS, written or implied.
    pub ignore_nulls: bool,
}

/// [ISO/IEC 9075-2:2023 §6.10] Which row of its frame a value function
/// takes the value of.
#[derive(Debug, Clone)]
pub enum Which {
    /// FIRST_VALUE.
    First,
    /// LAST_VALUE.
    Last,
    /// NTH_VALUE of the `<nth row>`, counted from the frame's last row if
    /// FROM LAST, else from its first.
    Nth {
        /// The `<nth row>`.
        row: Box<ValueExpression>,
        /// FROM LAST rather than FROM FIRST, written or implied.
        from_last: bool,
    },
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
    /// own parts, or its function's arguments, and those of its in-line
    /// window specification.
    pub(super) fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match &self.kind {
            WindowKind::RowNumber | WindowKind::Rank(_) => {}
            WindowKind::Aggregate(aggregate) => aggregate.for_each_child(f),
            WindowKind::Ntile(tiles) => f(tiles),
            WindowKind::Offset(o) => {
                f(&o.extent);
                o.default.iter().for_each(&mut *f);
            }
            WindowKind::Value(v) => {
                f(&v.arg);
                if let Which::Nth { row, .. } = &v.which {
                    f(row);
                }
            }
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

/// [ISO/IEC 9075-2:2023 §6.11] `<nested window function>` ::= `<nested row
/// number function>` | `<value_of expression at row>`.
#[derive(Debug, Clone)]
pub struct NestedWindow {
    /// The `<row marker>` it reads at.
    pub marker: RowMarker,
    /// The `<row marker delta>`, if written: whether `<minus sign>`, and
    /// the `<row marker offset>`.
    pub delta: Option<(bool, Box<ValueExpression>)>,
    /// Of VALUE_OF, the `<value expression>` and the `<value_of default
    /// value>` if written; none of ROW_NUMBER.
    pub value: Option<(Box<ValueExpression>, Option<Box<ValueExpression>>)>,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §6.11] `<row marker>`, the row of a partition a
/// nested window function reads [§4.17.15].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowMarker {
    /// BEGIN_PARTITION: its partition's first row.
    BeginPartition,
    /// BEGIN_FRAME: its frame's first row.
    BeginFrame,
    /// CURRENT_ROW: the row its window function is computed for.
    CurrentRow,
    /// FRAME_ROW: the row of the frame its set function takes.
    FrameRow,
    /// END_FRAME: its frame's last row.
    EndFrame,
    /// END_PARTITION: its partition's last row.
    EndPartition,
}

impl NestedWindow {
    /// Calls `f` with each value expression it is made of: its offset, and
    /// VALUE_OF's value expression and default.
    pub(super) fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        if let Some((_, offset)) = &self.delta {
            f(offset);
        }
        if let Some((value, default)) = &self.value {
            f(value);
            if let Some(default) = default {
                f(default);
            }
        }
    }
}
