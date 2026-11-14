//! [ISO/IEC 9075-2:2023 §7.17 GR6, GR7] The rows a query expression's
//! `<result offset clause>` and `<fetch first clause>` keep of its rows,
//! in their order.

use super::{Expr, render_expr};
use crate::value::Value;

/// [ISO/IEC 9075-2:2023 §7.17 GR6, GR7] OFFSET and FETCH FIRST, each
/// count an exact number of scale 0 over no columns, evaluated once as
/// the rows are first read.
#[derive(Debug, Clone)]
pub struct Limit {
    /// The `<offset row count>`, if written: the rows passed over first
    /// \[GR6\].
    pub offset: Option<Expr>,
    /// The `<fetch first row count>`, if a `<fetch first clause>` is
    /// written: the most rows kept after \[GR7\].
    pub fetch: Option<Expr>,
}

impl Limit {
    /// Its counts.
    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        self.offset.iter().chain(&self.fetch)
    }

    /// It, each count replaced by `f` of it.
    #[must_use]
    pub fn map_exprs(self, f: &mut dyn FnMut(Expr) -> Expr) -> Limit {
        Limit { offset: self.offset.map(&mut *f), fetch: self.fetch.map(f) }
    }

    /// The most rows it keeps, if its FETCH FIRST count is a constant row
    /// count.
    pub fn most(&self) -> Option<usize> {
        let Some(Expr::Const(Value::Exact(d))) = &self.fetch else { return None };
        if d.scale != 0 {
            return None;
        }
        usize::try_from(d.units).ok()
    }

    /// It as EXPLAIN shows it: "Limit", then "offset" and "fetch" with
    /// each count written.
    pub fn render(&self) -> String {
        let counts = [("offset", &self.offset), ("fetch", &self.fetch)];
        let written = counts.iter().filter_map(|(word, count)| count.as_ref().map(|e| format!(" {word} {}", render_expr(e, &[]))));
        std::iter::once(String::from("Limit")).chain(written).collect()
    }
}
