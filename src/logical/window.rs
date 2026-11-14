//! [ISO/IEC 9075-2:2023 §7.15, §6.10] The window functions of one window
//! partitioning and ordering, computed over the rows of a query's table
//! expression or of its groups: each row given with the value of each
//! function for it appended.

use super::{AggCall, Expr, SortKey, column_name, render_expr, render_keys};
use crate::value::Value;

/// [ISO/IEC 9075-2:2023 §7.15] The window functions over the rows of
/// `input`, of plans `P`, partitioned by `partition` and ordered by
/// `order`: each row of it is a row of `input`, then one column for each
/// of `calls`, in order.
#[derive(Debug, Clone)]
pub struct Window<P> {
    /// The window partitioning columns of `input`, each its sort key if
    /// it has a collation not the default [§7.15 GR3].
    pub partition: Vec<usize>,
    /// The window ordering, over the columns of `input` [§7.15 GR4].
    pub order: Vec<SortKey>,
    /// The functions computed, each over its own frame.
    pub calls: Vec<WindowCall>,
    /// The rows each function is computed over.
    pub input: P,
}

/// [ISO/IEC 9075-2:2023 §6.10] A window function of a `Window`.
#[derive(Debug, Clone)]
pub struct WindowCall {
    /// What it computes.
    pub kind: WindowKind,
    /// The rows of its frame [§7.15 GR5]: of an aggregate or a value
    /// function, those it is computed over.
    pub frame: Frame,
    /// The call as written, its column's display name.
    pub label: String,
    /// [ISO/IEC 9075-2:2023 §6.11] Of an aggregate, the nested window
    /// functions of its argument, inner first. The `k`th is read in its
    /// argument, and in the value expressions and defaults of those after
    /// it, as the column after the window's input's `k` columns on.
    pub nested: Vec<Nested>,
}

impl WindowCall {
    /// Its expressions but its frame's offsets: its kind's, and its nested
    /// window functions'.
    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        self.kind.exprs().into_iter().chain(self.nested.iter().flat_map(Nested::exprs))
    }

    /// Its expressions, as `exprs` gives them, each replaced by `f` of it.
    pub fn map_exprs(&mut self, f: &mut dyn FnMut(Expr) -> Expr) {
        self.kind.map_exprs(f);
        self.nested.iter_mut().for_each(|n| n.map_exprs(f));
    }
}

/// [ISO/IEC 9075-2:2023 §6.11] A `<nested window function>`: of a row
/// marker, plus or minus an offset, ROW_NUMBER, or VALUE_OF a value
/// expression at it.
#[derive(Debug, Clone)]
pub struct Nested {
    /// The row it reads at.
    pub marker: Marker,
    /// The `<row marker delta>`, if written: whether subtracted, and the
    /// offset, over no columns.
    pub delta: Option<(bool, Expr)>,
    /// Of VALUE_OF, the value expression, read at the row the marker and
    /// delta give, and the default, read at the frame's row, each of
    /// VALUE_OF's type \[SR3\]; none of ROW_NUMBER.
    pub value: Option<(Expr, Expr)>,
}

/// [ISO/IEC 9075-2:2023 §6.11] A `<row marker>` [§4.17.15].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// BEGIN_PARTITION: the partition's first row.
    BeginPartition,
    /// BEGIN_FRAME: the frame's first row.
    BeginFrame,
    /// CURRENT_ROW: the row the window function is computed for.
    CurrentRow,
    /// FRAME_ROW: the frame's row its set function takes.
    FrameRow,
    /// END_FRAME: the frame's last row.
    EndFrame,
    /// END_PARTITION: the partition's last row.
    EndPartition,
}

impl Marker {
    /// Whether its row is of the row the window function is computed for
    /// or of its frame, not of the frame's row and the partition alone.
    pub fn of_current(self) -> bool {
        match self {
            Marker::BeginFrame | Marker::CurrentRow | Marker::EndFrame => true,
            Marker::BeginPartition | Marker::FrameRow | Marker::EndPartition => false,
        }
    }

    /// It as written.
    pub fn render(self) -> &'static str {
        match self {
            Marker::BeginPartition => "BEGIN_PARTITION",
            Marker::BeginFrame => "BEGIN_FRAME",
            Marker::CurrentRow => "CURRENT_ROW",
            Marker::FrameRow => "FRAME_ROW",
            Marker::EndFrame => "END_FRAME",
            Marker::EndPartition => "END_PARTITION",
        }
    }
}

impl Nested {
    /// Its expressions: its offset, and VALUE_OF's value expression and
    /// default.
    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        self.delta.iter().map(|(_, e)| e).chain(self.value.iter().flat_map(|(v, d)| [v, d]))
    }

    /// Its expressions, as `exprs` gives them, each replaced by `f` of it.
    pub fn map_exprs(&mut self, f: &mut dyn FnMut(Expr) -> Expr) {
        let mut put = |e: &mut Expr| *e = f(std::mem::replace(e, Expr::Const(Value::Null)));
        if let Some((_, offset)) = &mut self.delta {
            put(offset);
        }
        if let Some((value, default)) = &mut self.value {
            put(value);
            put(default);
        }
    }
}

/// [ISO/IEC 9075-2:2023 §6.10] What a window function computes.
#[derive(Debug, Clone)]
pub enum WindowKind {
    /// ROW_NUMBER: its row's place in its partition [§9.23 SR5f].
    RowNumber,
    /// RANK: 1 plus the rows of the peer groups before its own [§9.23 SR5d].
    Rank,
    /// DENSE_RANK: 1 plus the peer groups before its own [§9.23 SR5e].
    DenseRank,
    /// PERCENT_RANK: (RANK - 1) / (rows - 1), or 0 of one row [§9.23 SR5g].
    PercentRank,
    /// CUME_DIST: the rows up to its peer group's last, over the rows
    /// [§9.23 SR5h].
    CumeDist,
    /// An `<aggregate function>`, its argument over the columns of the
    /// window's input, computed over its frame [§7.15 GR5].
    Aggregate(AggCall),
    /// NTILE of the number of tiles, over no columns [§6.10 GR1a].
    Ntile(Expr),
    /// LEAD or LAG [§6.10 GR1b].
    Offset(Offset),
    /// FIRST_VALUE, LAST_VALUE or NTH_VALUE, over its frame [§6.10 GR1c,
    /// GR1d].
    Value(Valued),
}

/// [ISO/IEC 9075-2:2023 §6.10 GR1b] LEAD or LAG: the extent of the row
/// `offset` rows after or before the current row in its partition, else
/// the default.
#[derive(Debug, Clone)]
pub struct Offset {
    /// LEAD rather than LAG.
    pub lead: bool,
    /// The extent, over the columns of the window's input.
    pub extent: Expr,
    /// How many rows after or before [§9.23 SR5j ii].
    pub offset: u32,
    /// The default, over the columns of the window's input, of the
    /// extent's type [§9.23 SR5j iii].
    pub default: Expr,
    /// IGNORE NULLS: rows whose extent is null are not counted.
    pub ignore_nulls: bool,
}

/// [ISO/IEC 9075-2:2023 §6.10 GR1c, GR1d] FIRST_VALUE, LAST_VALUE or
/// NTH_VALUE: the argument of a row of the frame.
#[derive(Debug, Clone)]
pub struct Valued {
    /// Which row of the frame.
    pub pick: Pick,
    /// The argument, over the columns of the window's input.
    pub arg: Expr,
    /// IGNORE NULLS: rows whose argument is null are not counted.
    pub ignore_nulls: bool,
}

/// [ISO/IEC 9075-2:2023 §6.10] Which row of its frame a value function
/// takes.
#[derive(Debug, Clone)]
pub enum Pick {
    /// FIRST_VALUE: the first.
    First,
    /// LAST_VALUE: the last.
    Last,
    /// NTH_VALUE: the `row`th, over no columns, from the last if
    /// `from_last`.
    Nth {
        /// The `<nth row>`.
        row: Expr,
        /// FROM LAST.
        from_last: bool,
    },
}

impl WindowKind {
    /// Its expressions: its argument, extent, default and counts.
    pub fn exprs(&self) -> Vec<&Expr> {
        match self {
            WindowKind::RowNumber | WindowKind::Rank | WindowKind::DenseRank | WindowKind::PercentRank | WindowKind::CumeDist => Vec::new(),
            WindowKind::Aggregate(a) => a.arg.iter().collect(),
            WindowKind::Ntile(tiles) => vec![tiles],
            WindowKind::Offset(o) => vec![&o.extent, &o.default],
            WindowKind::Value(v) => match &v.pick {
                Pick::First | Pick::Last => vec![&v.arg],
                Pick::Nth { row, .. } => vec![&v.arg, row],
            },
        }
    }

    /// Its expressions, as `exprs` gives them, each replaced by `f` of it.
    pub fn map_exprs(&mut self, f: &mut dyn FnMut(Expr) -> Expr) {
        let mut put = |e: &mut Expr| *e = f(std::mem::replace(e, Expr::Const(Value::Null)));
        match self {
            WindowKind::RowNumber | WindowKind::Rank | WindowKind::DenseRank | WindowKind::PercentRank | WindowKind::CumeDist => {}
            WindowKind::Aggregate(a) => a.arg.iter_mut().for_each(put),
            WindowKind::Ntile(tiles) => put(tiles),
            WindowKind::Offset(o) => {
                put(&mut o.extent);
                put(&mut o.default);
            }
            WindowKind::Value(v) => {
                put(&mut v.arg);
                if let Pick::Nth { row, .. } = &mut v.pick {
                    put(row);
                }
            }
        }
    }

    /// Whether it is computed over a frame: an aggregate's or a value
    /// function's.
    pub fn framed(&self) -> bool {
        match self {
            WindowKind::Aggregate(_) | WindowKind::Value(_) => true,
            WindowKind::RowNumber
            | WindowKind::Rank
            | WindowKind::DenseRank
            | WindowKind::PercentRank
            | WindowKind::CumeDist
            | WindowKind::Ntile(_)
            | WindowKind::Offset(_) => false,
        }
    }
}

/// [ISO/IEC 9075-2:2023 §7.15] A window frame: units, bounds and
/// exclusion, the defaults made explicit [SR11, GR5a].
#[derive(Debug, Clone)]
pub struct Frame {
    /// ROWS, RANGE or GROUPS.
    pub units: Units,
    /// Its first bound.
    pub start: Bound,
    /// Its last bound.
    pub end: Bound,
    /// What it leaves out of the rows its bounds take in.
    pub exclusion: Exclusion,
}

/// [ISO/IEC 9075-2:2023 §7.15] `<window frame units>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Units {
    /// ROWS: rows counted [GR5b i 2].
    Rows,
    /// RANGE: rows by the value of the single sort key [GR5b i 1], or
    /// peer groups of no offset.
    Range,
    /// GROUPS: peer groups counted [GR5b i 3].
    Groups,
}

/// [ISO/IEC 9075-2:2023 §7.15] A `<window frame bound>`, its offset over
/// no columns.
#[derive(Debug, Clone)]
pub enum Bound {
    /// UNBOUNDED PRECEDING: the partition's first row.
    UnboundedPreceding,
    /// The offset PRECEDING.
    Preceding(Expr),
    /// CURRENT ROW.
    CurrentRow,
    /// The offset FOLLOWING.
    Following(Expr),
    /// UNBOUNDED FOLLOWING: the partition's last row.
    UnboundedFollowing,
}

/// [ISO/IEC 9075-2:2023 §7.15 GR5b ii] `<window frame exclusion>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exclusion {
    /// EXCLUDE NO OTHERS.
    NoOthers,
    /// EXCLUDE CURRENT ROW.
    CurrentRow,
    /// EXCLUDE GROUP: the current row and its peers.
    Group,
    /// EXCLUDE TIES: the current row's peers, not itself.
    Ties,
}

impl Frame {
    /// The frame of no window framing clause: the whole partition, or of
    /// an ordering, the rows up to the current row's last peer [§7.15 GR5a].
    pub fn implied(ordered: bool) -> Frame {
        let end = if ordered { Bound::CurrentRow } else { Bound::UnboundedFollowing };
        Frame { units: Units::Range, start: Bound::UnboundedPreceding, end, exclusion: Exclusion::NoOthers }
    }

    /// Its offsets, of a bound with one.
    pub fn offsets(&self) -> impl Iterator<Item = &Expr> {
        [&self.start, &self.end].into_iter().filter_map(Bound::offset)
    }

    /// It as text, as a `<window frame clause>` is written.
    fn render(&self) -> String {
        let units = match self.units {
            Units::Rows => "ROWS",
            Units::Range => "RANGE",
            Units::Groups => "GROUPS",
        };
        let exclusion = match self.exclusion {
            Exclusion::NoOthers => "",
            Exclusion::CurrentRow => " EXCLUDE CURRENT ROW",
            Exclusion::Group => " EXCLUDE GROUP",
            Exclusion::Ties => " EXCLUDE TIES",
        };
        format!("{units} BETWEEN {} AND {}{exclusion}", self.start.render(), self.end.render())
    }
}

impl Bound {
    /// Its offset, if it has one.
    pub fn offset(&self) -> Option<&Expr> {
        match self {
            Bound::Preceding(e) | Bound::Following(e) => Some(e),
            Bound::UnboundedPreceding | Bound::CurrentRow | Bound::UnboundedFollowing => None,
        }
    }

    fn render(&self) -> String {
        match self {
            Bound::UnboundedPreceding => "UNBOUNDED PRECEDING".to_owned(),
            Bound::Preceding(e) => format!("{} PRECEDING", render_expr(e, &[])),
            Bound::CurrentRow => "CURRENT ROW".to_owned(),
            Bound::Following(e) => format!("{} FOLLOWING", render_expr(e, &[])),
            Bound::UnboundedFollowing => "UNBOUNDED FOLLOWING".to_owned(),
        }
    }
}

impl<P> Window<P> {
    /// It apart from its input, and its input.
    pub fn split(self) -> (Window<()>, P) {
        let Window { partition, order, calls, input } = self;
        (Window { partition, order, calls, input: () }, input)
    }

    /// It over `f` of its input, the rest copied.
    pub fn with_input<Q>(&self, f: impl FnOnce(&P) -> Q) -> Window<Q> {
        Window { partition: self.partition.clone(), order: self.order.clone(), calls: self.calls.clone(), input: f(&self.input) }
    }

    /// The expressions of its calls: their arguments, counts and frames'
    /// offsets.
    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        self.calls.iter().flat_map(|c| c.exprs().chain(c.frame.offsets()))
    }

    /// Its calls' expressions, each replaced by `f` of it.
    pub fn map_exprs(&mut self, f: &mut dyn FnMut(Expr) -> Expr) {
        for call in &mut self.calls {
            call.map_exprs(f);
            for bound in [&mut call.frame.start, &mut call.frame.end] {
                if let Bound::Preceding(e) | Bound::Following(e) = bound {
                    *e = f(std::mem::replace(e, Expr::Const(Value::Null)));
                }
            }
        }
    }

    /// Whether computing it can raise a run-time error: an aggregate's, an
    /// expression's of a call, or a frame's offset's (`exec/window.rs`).
    pub fn can_raise(&self) -> bool {
        self.calls.iter().any(|c| matches!(c.kind, WindowKind::Aggregate(_))) || self.exprs().next().is_some()
    }

    /// Its own columns' display names, after its input's: its calls'.
    pub fn names(&self) -> impl Iterator<Item = String> + '_ {
        self.calls.iter().map(|c| c.label.clone())
    }

    /// It as text, its input's columns named `names`: its partitioning,
    /// ordering and calls, each with its frame.
    pub fn render(&self, names: &[String]) -> String {
        let partition: Vec<String> = self.partition.iter().map(|&c| column_name(names, c)).collect();
        let calls: Vec<String> = self
            .calls
            .iter()
            .map(|c| if c.kind.framed() { format!("{} ({})", c.label, c.frame.render()) } else { c.label.clone() })
            .collect();
        format!(
            "Window partition by [{}] order by [{}] computing [{}]",
            partition.join(", "),
            render_keys(&self.order, names),
            calls.join(", ")
        )
    }
}

impl Window<()> {
    /// The same window functions over `input`.
    pub fn over<Q>(self, input: Q) -> Window<Q> {
        let Window { partition, order, calls, input: () } = self;
        Window { partition, order, calls, input }
    }
}
