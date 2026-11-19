//! [ISO/IEC 9075-2:2023 §7.17 GR6, GR7] The rows a query expression's
//! `<result offset clause>` and `<fetch first clause>` keep of its rows,
//! in their order.

mod search;

pub use search::{Hints, Search, SearchTarget, Strategy};

use super::{Expr, SortKey, render_expr};
use crate::value::Value;

/// [ISO/IEC 9075-2:2023 §7.17 GR6, GR7] OFFSET and FETCH FIRST, each
/// count over no columns, evaluated once as the rows are first read.
#[derive(Debug, Clone)]
pub struct Limit {
    /// The `<offset row count>`, if written, an exact number of scale 0:
    /// the rows passed over first \[GR6\].
    pub offset: Option<Expr>,
    /// The `<fetch first clause>`, if written \[GR7\].
    pub fetch: Option<Fetch>,
}

/// [ISO/IEC 9075-2:2023 §7.17 GR7] A `<fetch first clause>`: the most
/// rows kept after OFFSET's, and of WITH TIES their peers too.
#[derive(Debug, Clone)]
pub struct Fetch {
    /// Its `<fetch first row count>`, an exact number of scale 0, 1 if
    /// none is written \[GR7b iii\]; or of `percent`, the value of its
    /// `<fetch first percentage>`, a number.
    pub count: Expr,
    /// [GR7b ii] Its count is a percentage of the rows before OFFSET.
    pub percent: bool,
    /// [GR7d i] Of WITH TIES, the keys whose peers of the last row kept are
    /// kept too: the keys of its rows' sort, columns of its rows, none until
    /// that sort is made (`binder/order_by.rs`). None of ONLY.
    pub ties: Option<Vec<SortKey>>,
    /// \[EXTENSION\] APPROX, EXACT or neither, and what APPROX asks.
    pub search: Search,
    /// \[EXTENSION\] The vector search hints written after it (R13.4),
    /// boxed, as they are seldom written and a plan is kept small.
    pub hints: Box<Hints>,
}

impl Limit {
    /// Its counts.
    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        self.offset.iter().chain(self.fetch.iter().map(|f| &f.count))
    }

    /// It, each count replaced by `f` of it.
    #[must_use]
    pub fn map_exprs(self, f: &mut dyn FnMut(Expr) -> Expr) -> Limit {
        let offset = self.offset.map(&mut *f);
        Limit { offset, fetch: self.fetch.map(|fetch| Fetch { count: f(fetch.count), ..fetch }) }
    }

    /// The most rows it keeps, if its FETCH FIRST count is a constant row
    /// count, of ONLY.
    pub fn most(&self) -> Option<usize> {
        let Some(Fetch { count: Expr::Const(Value::Exact(d)), percent: false, ties: None, .. }) = &self.fetch else { return None };
        if d.scale != 0 {
            return None;
        }
        usize::try_from(d.units).ok()
    }

    /// The count of its FETCH FIRST, if a constant row count, of ONLY or
    /// WITH TIES: WITH TIES keeps those rows and then their peers.
    fn counted(&self) -> Option<usize> {
        let Some(Fetch { count: Expr::Const(Value::Exact(d)), percent: false, .. }) = &self.fetch else { return None };
        if d.scale != 0 {
            return None;
        }
        usize::try_from(d.units).ok()
    }

    /// The first of its input's rows it reads, if its counts are
    /// constants: OFFSET's and FETCH FIRST's, of ONLY or WITH TIES, whose
    /// peers of the last are read after them (RULES.md D135). Of no FETCH
    /// FIRST, or of PERCENT, whose count needs every row, its input is
    /// read to its end.
    pub fn read_rows(&self) -> Option<usize> {
        let passed = match &self.offset {
            None => Some(0),
            Some(Expr::Const(Value::Exact(d))) => (d.scale == 0).then(|| usize::try_from(d.units).ok()).flatten(),
            Some(_) => None,
        };
        passed?.checked_add(self.counted()?)
    }

    /// It as EXPLAIN shows it: "Limit", then "offset" and "fetch" with
    /// each count written, "percent" after a percentage and "with ties"
    /// after a count of WITH TIES.
    pub fn render(&self) -> String {
        let mut out = String::from("Limit");
        if let Some(e) = &self.offset {
            out.push_str(" offset ");
            out.push_str(&render_expr(e, &[]));
        }
        if let Some(f) = &self.fetch {
            out.push_str(" fetch ");
            out.push_str(&render_expr(&f.count, &[]));
            if f.percent {
                out.push_str(" percent");
            }
            if f.ties.is_some() {
                out.push_str(" with ties");
            }
            match &f.search {
                Search::Unstated => {}
                Search::Exact => out.push_str(" exact"),
                Search::Approximate(_) => out.push_str(" approx"),
            }
        }
        out
    }
}

/// \[DESIGN\] Of a sort, or a scan in an order, the most rows of it read
/// that are asked for, the first in its order, as FETCH FIRST and OFFSET
/// ask, and of WITH TIES the peers of the last of them too
/// (`exec/limit.rs`): given it as it runs, none as planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Most {
    /// How many rows are asked for.
    pub rows: usize,
    /// The peers of the last are asked for too.
    pub ties: bool,
}
