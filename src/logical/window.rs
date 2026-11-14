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
    /// The rows of its frame [§7.15 GR5]: of an aggregate, those it is
    /// computed over.
    pub frame: Frame,
    /// The call as written, its column's display name.
    pub label: String,
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

    /// The expressions of its calls: their arguments and frames' offsets.
    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        self.calls.iter().flat_map(|c| {
            let arg = match &c.kind {
                WindowKind::Aggregate(a) => a.arg.as_ref(),
                WindowKind::RowNumber | WindowKind::Rank | WindowKind::DenseRank | WindowKind::PercentRank | WindowKind::CumeDist => None,
            };
            arg.into_iter().chain(c.frame.offsets())
        })
    }

    /// Its calls' expressions, each replaced by `f` of it.
    pub fn map_exprs(&mut self, f: &mut dyn FnMut(Expr) -> Expr) {
        for call in &mut self.calls {
            if let WindowKind::Aggregate(a) = &mut call.kind {
                a.arg = a.arg.take().map(&mut *f);
            }
            for bound in [&mut call.frame.start, &mut call.frame.end] {
                if let Bound::Preceding(e) | Bound::Following(e) = bound {
                    *e = f(std::mem::replace(e, Expr::Const(Value::Null)));
                }
            }
        }
    }

    /// Whether computing it can raise a run-time error: an aggregate's, or
    /// a frame's offset's (`exec/window.rs`).
    pub fn can_raise(&self) -> bool {
        self.calls.iter().any(|c| matches!(c.kind, WindowKind::Aggregate(_)) || c.frame.offsets().next().is_some())
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
            .map(|c| match &c.kind {
                WindowKind::Aggregate(_) => format!("{} ({})", c.label, c.frame.render()),
                WindowKind::RowNumber | WindowKind::Rank | WindowKind::DenseRank | WindowKind::PercentRank | WindowKind::CumeDist => {
                    c.label.clone()
                }
            })
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
