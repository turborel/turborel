//! The predicates of filters evaluated [ISO/IEC 9075-2:2023 §9.46 GR12]:
//! each True, False or Unknown, by SQL's three-valued logic.
//!
//! An exception of a `<JSON path wff>` within a predicate is caught: the
//! predicate it is of is Unknown [§4.48.5], strict mode's structural errors
//! too. Comparisons, like_regex and starts with test each item, or pair of
//! items, noting whether any is in error (ERR) and whether any succeeds
//! (FOUND). Implementation-dependent (UA073): if both, in lax mode, the
//! result is True; so a test stops at the first success in lax mode, and
//! at the first error in strict mode, where an error makes it Unknown.

use std::cmp::Ordering;

use super::{Evaluator, string};
use crate::json::Item;
use crate::json::path::ast::{Comparison, Expr, Mode, Predicate};
use crate::regex::Regex;
use crate::value::{Value, compare};

/// [§9.46 GR12] The value of a `<JSON path predicate>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Truth {
    /// True.
    True,
    /// False.
    False,
    /// Unknown.
    Unknown,
}

/// ERR and FOUND, as a test of items has set them.
#[derive(Debug, Clone, Copy, Default)]
struct Outcome {
    err: bool,
    found: bool,
}

impl Outcome {
    /// Whether no later item can change the result in mode `mode`.
    fn settled(self, mode: Mode) -> bool {
        match mode {
            Mode::Lax => self.found,
            Mode::Strict => self.err,
        }
    }

    /// [§9.46 GR12c iii 2 E, d ii 2 F, e ii 3 D] The result in mode `mode`.
    fn truth(self, mode: Mode) -> Truth {
        if mode == Mode::Strict && self.err {
            return Truth::Unknown;
        }
        if self.found {
            return Truth::True;
        }
        if self.err { Truth::Unknown } else { Truth::False }
    }
}

impl Evaluator<'_> {
    /// [§9.46 GR12] The value of `predicate`.
    pub(super) fn truth(&self, predicate: &Predicate) -> Truth {
        match predicate {
            Predicate::Exists(expr) => self.exists(expr),
            Predicate::Compare(left, op, right) => self.compare(left, *op, right),
            Predicate::LikeRegex(expr, regex) => self.like_regex(expr, regex),
            Predicate::StartsWith(whole, initial) => self.starts_with(whole, initial),
            // [GR12f] True if Unknown, else False.
            Predicate::IsUnknown(operand) => {
                if self.truth(operand) == Truth::Unknown {
                    Truth::True
                } else {
                    Truth::False
                }
            }
            Predicate::Not(operand) => not(self.truth(operand)),
            Predicate::And(left, right) => and(self.truth(left), self.truth(right)),
            Predicate::Or(left, right) => or(self.truth(left), self.truth(right)),
        }
    }

    /// [§9.46 GR12a] `<JSON exists path predicate>`: whether `expr` gives
    /// any item; Unknown of an exception.
    fn exists(&self, expr: &Expr) -> Truth {
        match self.wff(expr) {
            Err(_) => Truth::Unknown,
            Ok(seq) => {
                if seq.is_empty() {
                    Truth::False
                } else {
                    Truth::True
                }
            }
        }
    }

    /// [§9.46 GR12c] `<JSON comparison predicate>`: whether some item of
    /// `left` and some of `right` compare as `op` says.
    fn compare(&self, left: &Expr, op: Comparison, right: &Expr) -> Truth {
        let (Ok(a), Ok(b)) = (self.wff(left), self.wff(right)) else { return Truth::Unknown };
        let (a, b) = (self.lax_unwrap(a), self.lax_unwrap(b));
        let mut outcome = Outcome::default();
        for x in &a {
            for y in &b {
                match pair(x, y, op) {
                    Some(found) => outcome.found |= found,
                    None => outcome.err = true,
                }
                if outcome.settled(self.mode) {
                    return outcome.truth(self.mode);
                }
            }
        }
        outcome.truth(self.mode)
    }

    /// [§9.46 GR12d] `<JSON like_regex predicate>`: whether some item of
    /// `expr`, a character string, matches `regex`. \[DESIGN\] A match the
    /// matcher gives up on, past its steps, is an error of the item.
    fn like_regex(&self, expr: &Expr, regex: &Regex) -> Truth {
        let Ok(seq) = self.wff(expr) else { return Truth::Unknown };
        let mut outcome = Outcome::default();
        for each in &self.lax_unwrap(seq) {
            match string(each).map(|s| regex.is_match(s)) {
                Some(Ok(found)) => outcome.found |= found,
                Some(Err(_)) | None => outcome.err = true,
            }
            if outcome.settled(self.mode) {
                break;
            }
        }
        outcome.truth(self.mode)
    }

    /// [§9.46 GR12e] `<JSON starts with predicate>`: whether some item of
    /// `whole`, a character string, begins with the one character string
    /// `initial` gives.
    fn starts_with(&self, whole: &Expr, initial: &Expr) -> Truth {
        let (Ok(a), Ok(b)) = (self.wff(whole), self.wff(initial)) else { return Truth::Unknown };
        // [GR12e ii 2] "If B is not a character string": B, a sequence, is
        // to be of one item, a character string.
        let Some(prefix) = singleton_string(&b) else { return Truth::Unknown };
        let mut outcome = Outcome::default();
        for each in &self.lax_unwrap(a) {
            match string(each) {
                // Of characters: a prefix of UTF-8 is a prefix of its code
                // points.
                Some(s) => outcome.found |= s.starts_with(prefix),
                None => outcome.err = true,
            }
            if outcome.settled(self.mode) {
                break;
            }
        }
        outcome.truth(self.mode)
    }
}

/// The character string the sequence `seq` is, if one item, a string.
fn singleton_string(seq: &[Item]) -> Option<&str> {
    let [only] = seq else { return None };
    string(only)
}

/// [§9.46 GR12c iii 2 D] Whether `x` and `y` compare as `op` says; none
/// if they are not equality-comparable, for `==` and `!=`, or not
/// order-comparable, for the others [§4.48.3]: comparable if either is
/// the SQL/JSON null, or both are scalars SQL compares.
fn pair(x: &Item, y: &Item, op: Comparison) -> Option<bool> {
    match (x, y) {
        // [D III 1, D VII 1, D VIII 1] Both the SQL/JSON null.
        (Item::Null, Item::Null) => Some(matches!(op, Comparison::Equal | Comparison::LessOrEqual | Comparison::GreaterOrEqual)),
        // [D IV 1, D IV 2] One the SQL/JSON null: unequal, and no other.
        (Item::Null, _) | (_, Item::Null) => Some(op == Comparison::NotEqual),
        (Item::Scalar(a), Item::Scalar(b)) => order(a.value(), b.value()).map(|ordering| holds(op, ordering)),
        (Item::Array(_) | Item::Object(_), _) | (_, Item::Array(_) | Item::Object(_)) => None,
    }
}

/// [§9.46 GR12c iii 2 D] The order of two scalars, if comparable: of
/// character strings, "the Unicode codepoint collation", which Rust's
/// order of strings is; of others, SQL's.
fn order(a: &Value, b: &Value) -> Option<Ordering> {
    if let (Value::Char(x), Value::Char(y)) = (a, b) {
        return Some(x.as_str().cmp(y.as_str()));
    }
    compare(a, b)
}

/// [§9.46 GR12c iii 2 D III to VIII] Whether `op` holds of scalars in the
/// order `ordering`.
fn holds(op: Comparison, ordering: Ordering) -> bool {
    match op {
        Comparison::Equal => ordering == Ordering::Equal,
        Comparison::NotEqual => ordering != Ordering::Equal,
        Comparison::Less => ordering == Ordering::Less,
        Comparison::Greater => ordering == Ordering::Greater,
        Comparison::LessOrEqual => ordering != Ordering::Greater,
        Comparison::GreaterOrEqual => ordering != Ordering::Less,
    }
}

/// [§9.46 GR12g] `!`.
fn not(a: Truth) -> Truth {
    match a {
        Truth::True => Truth::False,
        Truth::False => Truth::True,
        Truth::Unknown => Truth::Unknown,
    }
}

/// [§9.46 GR12h] `&&`: False if either is, else Unknown if either is.
fn and(a: Truth, b: Truth) -> Truth {
    if a == Truth::False || b == Truth::False {
        return Truth::False;
    }
    if a == Truth::Unknown || b == Truth::Unknown { Truth::Unknown } else { Truth::True }
}

/// [§9.46 GR12i] `||`: True if either is, else Unknown if either is.
fn or(a: Truth, b: Truth) -> Truth {
    if a == Truth::True || b == Truth::True {
        return Truth::True;
    }
    if a == Truth::Unknown || b == Truth::Unknown { Truth::Unknown } else { Truth::False }
}

#[cfg(test)]
mod tests;
