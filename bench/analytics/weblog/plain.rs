//! The web log's columns that no query's speed turns on (`weblog.rs`):
//! each drawn from its own measured figures, the values it holds most
//! often with their shares and, for the rest, as many other values as
//! were counted, spread over its range. A column of what a visitor's
//! browser or a counter's site is draws by the user or the counter, so a
//! user's rows agree; the others draw by the row.

use super::random::{Listed, MILLION, Random, below, drawn};
use super::texts::{self, Shape};

/// What a column's value is drawn by.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// The user: a fact of the visitor or the browser.
    User,
    /// The counter: a fact of the site.
    Counter,
    /// The row alone.
    Row,
}

/// A column of whole numbers, as measured.
pub struct Plain {
    pub column: &'static str,
    pub by: By,
    /// The values it holds most often, each with its share.
    pub top: Listed,
    /// The least and the greatest value, and how many values there are
    /// beside the listed ones.
    pub rest: (i64, i64, u64),
}

impl Plain {
    /// The column `column`, drawn by `by`, of the table `top` and of
    /// `rest`.
    pub const fn new(column: &'static str, by: By, top: &'static [i64], rest: (i64, i64, u64)) -> Plain {
        Plain { column, by, top: Listed::new(top), rest }
    }

    /// The value `draw` falls on.
    pub fn value(&self, draw: u64) -> i64 {
        if let Some(value) = self.top.pick(draw) {
            return value;
        }
        let (low, high, others) = self.rest;
        let span = high.abs_diff(low);
        let nth = below(Random::new(draw).next(), others.max(1));
        let step = span.div_euclid(others.max(1)).max(1);
        low.wrapping_add((nth * step).min(span).cast_signed())
    }
}

/// A column of texts, as measured.
pub struct Worded {
    pub column: &'static str,
    pub by: By,
    pub shape: Shape,
    /// The texts it holds most often after the empty one: the octets of
    /// each, with its share of the rows that are not empty.
    pub top: Listed,
    /// How many texts there are beside the listed ones.
    pub others: u64,
}

impl Worded {
    /// The column `column`, drawn by `by`, of `shape`, of the table `top`
    /// and of `others` other texts.
    pub const fn new(column: &'static str, by: By, shape: Shape, top: &'static [i64], others: u64) -> Worded {
        Worded { column, by, shape, top: Listed::new(top), others }
    }

    /// Appends the text `draw` falls on to `out`, under `seed`.
    pub fn write(&self, out: &mut Vec<u8>, draw: u64, seed: u64) {
        let mut draws = Random::new(draw);
        let at = below(draws.next(), MILLION);
        if at < u64::from(self.shape.empty) {
            return;
        }
        let (length, number) = if let Some((place, length, _)) = self.top.placed(draws.next()) {
            (length.cast_unsigned() as usize, place as u64)
        } else {
            let number = below(draws.next(), self.others.max(1)) + self.top.len() as u64;
            (self.shape.length(drawn(seed, number, self.column.len() as u64)), number)
        };
        let mut words = Random::new(drawn(seed, number, length as u64));
        texts::text(out, b"", (length, number), (b' ', self.shape.wide), &mut words);
    }
}
