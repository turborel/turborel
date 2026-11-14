//! [ISO/IEC 9075-2:2023 §7.7, §7.9, §9.41] Row pattern recognition as
//! planned: the matches sought in each row pattern partition, the values a
//! definition or a measure reads of them, and the rows given.
//!
//! A definition or a measure is bound as an expression over a row of its
//! own values (`Slotted`), each a `PatternValue` read of the match: a row
//! pattern column reference or a classifier navigated to [§6.7 GR1b,
//! §6.27, §6.33 SR24d], the match number [§6.31] or an aggregate over a
//! variable's rows [§10.9 GR1a to c]. What such a value evaluates in a row
//! of the input is an expression over that row, then the classifier of
//! that row and the match number as two columns more.

use super::{AggCall, Cond, Expr, SortKey, column_name, render_keys};
use crate::pattern::Pattern;
use crate::value::Value;

/// [ISO/IEC 9075-2:2023 §7.7] A `<row pattern recognition clause>` over the
/// rows of `input`, of plans `P`. Each row of it is a row of `input`, then
/// each measure's value. Of ONE ROW PER MATCH that row is the match's
/// first, its row pattern partitioning columns' values not distinct from
/// those of every row of its partition [GR11a i]; of ALL ROWS PER MATCH it
/// is the row given \[GR11b\]. The columns of the row pattern output table
/// are some of these in the order of §7.7 SR7 to SR10, which a projection
/// over it gives.
#[derive(Debug, Clone)]
pub struct Recognize<P> {
    /// The row pattern partitioning columns of `input` \[GR1\], each its sort
    /// key if it has a collation not the default.
    pub partition: Vec<usize>,
    /// The order of the rows of each partition \[GR3\], over the columns of
    /// `input`.
    pub order: Vec<SortKey>,
    /// The matches sought.
    pub matching: Matching,
    /// The `<row pattern measure expression>`s, in order.
    pub measures: Slotted<Vec<Expr>>,
    /// The `<measure name>`s, in order.
    pub names: Vec<String>,
    /// The `<row pattern rows per match>`.
    pub rows: RowsPerMatch,
    /// The rows sought in.
    pub input: P,
}

/// [ISO/IEC 9075-2:2023 §7.7 GR11] What rows a match gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowsPerMatch {
    /// ONE ROW PER MATCH.
    One,
    /// ALL ROWS PER MATCH SHOW EMPTY MATCHES.
    AllShowEmpty,
    /// ALL ROWS PER MATCH OMIT EMPTY MATCHES.
    AllOmitEmpty,
    /// ALL ROWS PER MATCH WITH UNMATCHED ROWS.
    AllWithUnmatched,
}

/// [ISO/IEC 9075-2:2023 §7.9] A `<row pattern common syntax>` as planned:
/// the pattern, its variables, what each primary variable is defined as,
/// and where the next match is sought after one.
#[derive(Debug, Clone)]
pub struct Matching {
    /// The `<row pattern>`, its primary variables numbered as `variables`
    /// numbers them.
    pub pattern: Pattern,
    /// The row pattern variables: the primary ones first, numbered as
    /// `pattern` numbers them, then the union ones of SUBSET, then the
    /// universal row pattern variable [§7.9 SR18].
    pub variables: Vec<Variable>,
    /// Of each primary row pattern variable, its `<row pattern definition
    /// search condition>`, none for TRUE [§7.9 SR17].
    pub defines: Vec<Option<Slotted<Cond>>>,
    /// The `<row pattern skip to>`.
    pub skip: Skip,
    /// [ISO/IEC 9075-2:2023 §7.15 GR6d iii 2 B] Of a window, SEEK: the
    /// match sought from each row of the frame on, not from its first
    /// alone.
    pub seek: bool,
}

/// [ISO/IEC 9075-2:2023 §7.9 SR4 to SR18] A row pattern variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    /// Its name, as CLASSIFIER gives it [§6.33].
    pub name: String,
    /// The primary row pattern variables it is the union of, in order: of
    /// a primary one, itself alone [§7.9 SR16, SR18].
    pub components: Vec<usize>,
}

/// [ISO/IEC 9075-2:2023 §7.9] `<row pattern skip to>`, SKIP TO read as SKIP
/// TO LAST \[SR23b\].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// SKIP TO NEXT ROW.
    NextRow,
    /// SKIP PAST LAST ROW.
    PastLastRow,
    /// SKIP TO FIRST of the row pattern variable numbered so.
    First(usize),
    /// SKIP TO LAST of the row pattern variable numbered so.
    Last(usize),
}

/// An expression `expr` over a row of the values `values` reads of a
/// match, in order.
#[derive(Debug, Clone)]
pub struct Slotted<E> {
    /// The expression, its columns the values.
    pub expr: E,
    /// The values, each read of the match.
    pub values: Vec<PatternValue>,
}

/// A value read of a row pattern match: of its current row, or of a row
/// navigated to, or of the rows mapped to a variable.
#[derive(Debug, Clone)]
pub enum PatternValue {
    /// [ISO/IEC 9075-2:2023 §6.27, §6.7 GR1b] A `<row pattern navigation
    /// operation>`: `expr`, over a row of the input and its classifier,
    /// evaluated in the row navigated to; null if there is none \[GR11\]. A
    /// `<classifier function>` not in one is one of its own [§6.33 SR24d].
    Navigated(Box<Navigated>),
    /// [ISO/IEC 9075-2:2023 §6.31] MATCH_NUMBER ( ): the match's number in
    /// its partition, from 1.
    MatchNumber,
    /// [ISO/IEC 9075-2:2023 §10.9 GR1a to c] An aggregate over the rows
    /// mapped to a row pattern variable.
    Aggregate(Box<PatternAggregate>),
}

/// [ISO/IEC 9075-2:2023 §6.27] A `<row pattern navigation: compound>`.
#[derive(Debug, Clone)]
pub struct Navigated {
    /// The value expression, over a row of the input then its classifier.
    pub expr: Expr,
    /// The row pattern variable it navigates the rows of \[SR4\].
    pub variable: usize,
    /// FINAL: all the rows mapped to it, not those up to the current row
    /// alone \[GR6\].
    pub final_: bool,
    /// LAST: counted from the last row mapped to it, not the first.
    pub last: bool,
    /// The `<logical offset>`, over no columns \[GR2\].
    pub logical: Expr,
    /// NEXT: the physical offset counts forward, not back.
    pub next: bool,
    /// The `<physical offset>`, over no columns \[GR2\].
    pub physical: Expr,
}

/// [ISO/IEC 9075-2:2023 §10.9 GR1a to c] An aggregate in row pattern
/// matching.
#[derive(Debug, Clone)]
pub struct PatternAggregate {
    /// The aggregate, its argument and filter over a row of the input then
    /// its classifier; COUNT of no argument counting the rows.
    pub call: AggCall,
    /// The row pattern variable whose rows it aggregates, the universal one
    /// if it names none [GR1a ii].
    pub variable: usize,
    /// FINAL: all the rows mapped to it, not those up to the current row
    /// alone [§4.23.2].
    pub final_: bool,
}

/// [ISO/IEC 9075-2:2023 §7.15] A window's row pattern recognition: the
/// matches each frame is narrowed to, and its measures, read by its
/// `<window row pattern measure>`s.
#[derive(Debug, Clone)]
pub struct WindowMatching {
    /// The matches sought, in each row's full frame.
    pub matching: Matching,
    /// The `<row pattern measure expression>`s, in order.
    pub measures: Slotted<Vec<Expr>>,
    /// The `<measure name>`s, in order.
    pub names: Vec<String>,
}

/// `e` replaced by `f` of it.
fn put(e: &mut Expr, f: &mut dyn FnMut(Expr) -> Expr) {
    *e = f(std::mem::replace(e, Expr::Const(Value::Null)));
}

impl PatternValue {
    /// Its expressions: a navigation's value expression and offsets, an
    /// aggregate's argument and filter.
    pub fn exprs(&self) -> Vec<&Expr> {
        match self {
            PatternValue::Navigated(n) => vec![&n.expr, &n.logical, &n.physical],
            PatternValue::Aggregate(a) => a.call.exprs(),
            PatternValue::MatchNumber => Vec::new(),
        }
    }

    /// Its expressions, as `exprs` gives them, each replaced by `f` of it.
    pub fn map_exprs(&mut self, f: &mut dyn FnMut(Expr) -> Expr) {
        match self {
            PatternValue::Navigated(n) => {
                put(&mut n.expr, f);
                put(&mut n.logical, f);
                put(&mut n.physical, f);
            }
            PatternValue::Aggregate(a) => a.call = a.call.clone().map_exprs(f),
            PatternValue::MatchNumber => {}
        }
    }
}

impl Slotted<Vec<Expr>> {
    /// Its expressions and those of its values.
    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        self.expr.iter().chain(self.values.iter().flat_map(PatternValue::exprs))
    }

    /// Its expressions and those of its values, each replaced by `f` of it.
    pub fn map_exprs(&mut self, f: &mut dyn FnMut(Expr) -> Expr) {
        self.expr.iter_mut().for_each(|e| put(e, f));
        self.values.iter_mut().for_each(|v| v.map_exprs(f));
    }
}

impl Matching {
    /// The expressions of its definitions' values, then the conditions of
    /// its definitions.
    pub fn parts(&self) -> (impl Iterator<Item = &Expr>, impl Iterator<Item = &Cond>) {
        let defined = self.defines.iter().flatten();
        (defined.clone().flat_map(|d| d.values.iter().flat_map(PatternValue::exprs)), defined.map(|d| &d.expr))
    }

    /// Its definitions' conditions and values' expressions, each
    /// expression replaced by `f` of it.
    pub fn map_exprs(&mut self, f: &mut dyn FnMut(Expr) -> Expr) {
        for d in self.defines.iter_mut().flatten() {
            d.expr = d.expr.clone().map_exprs(f);
            d.values.iter_mut().for_each(|v| v.map_exprs(f));
        }
    }

    /// The `<row pattern skip to>` and `<row pattern>` as text, the
    /// variables by name.
    fn render(&self) -> String {
        let name = |v: usize| self.variables.get(v).map_or_else(|| format!("#{v}"), |v| v.name.clone());
        let skip = match self.skip {
            Skip::NextRow => "skip to next row".to_owned(),
            Skip::PastLastRow => "skip past last row".to_owned(),
            Skip::First(v) => format!("skip to first {}", name(v)),
            Skip::Last(v) => format!("skip to last {}", name(v)),
        };
        format!("pattern ({}) {skip}", rendered(&self.pattern, name))
    }
}

/// What is left to write of a pattern as text: a pattern, or text.
enum Written<'a> {
    /// A pattern, written as `rendered` writes it.
    Pattern(&'a Pattern),
    /// Text, written as it is.
    Text(String),
}

/// `pattern` as text, its variables named by `name`, written from a stack
/// of its own, so that a pattern however deep needs no recursion (RULES.md
/// R-CF-6).
fn rendered(pattern: &Pattern, name: impl Fn(usize) -> String) -> String {
    let (mut out, mut stack) = (String::new(), vec![Written::Pattern(pattern)]);
    while let Some(next) = stack.pop() {
        let p = match next {
            Written::Text(t) => {
                out.push_str(&t);
                continue;
            }
            Written::Pattern(p) => p,
        };
        match p {
            Pattern::Variable(v) => out.push_str(&name(*v)),
            Pattern::Start => out.push('^'),
            Pattern::End => out.push('$'),
            Pattern::Empty => out.push_str("()"),
            Pattern::Concat(ps) => listed(&mut stack, "(", ps, " "),
            Pattern::Alternation(ps) => listed(&mut stack, "(", ps, " | "),
            Pattern::Permute(ps) => listed(&mut stack, "PERMUTE (", ps, ", "),
            Pattern::Excluded(q) => stack.extend([Written::Text(" -}".to_owned()), Written::Pattern(q), Written::Text("{- ".to_owned())]),
            Pattern::Repeat { pattern: body, min, max, reluctant } => {
                let max = max.map_or_else(String::new, |m| m.to_string());
                let reluctant = if *reluctant { "?" } else { "" };
                stack.extend([Written::Text(format!("{{{min},{max}}}{reluctant}")), Written::Pattern(body)]);
            }
        }
    }
    out
}

/// Of `ps` written between `open` and a closing parenthesis, `sep` between
/// each two, what is left to write pushed on `stack`, last first, so that
/// each comes off it in order.
fn listed<'a>(stack: &mut Vec<Written<'a>>, open: &str, ps: &'a [Pattern], sep: &str) {
    stack.push(Written::Text(")".to_owned()));
    for (k, q) in ps.iter().enumerate().rev() {
        stack.push(Written::Pattern(q));
        if k > 0 {
            stack.push(Written::Text(sep.to_owned()));
        }
    }
    stack.push(Written::Text(open.to_owned()));
}

impl<P> Recognize<P> {
    /// It apart from its input, and its input.
    pub fn split(self) -> (Recognize<()>, P) {
        let Recognize { partition, order, matching, measures, names, rows, input } = self;
        (Recognize { partition, order, matching, measures, names, rows, input: () }, input)
    }

    /// It over `f` of its input, the rest copied.
    pub fn with_input<Q>(&self, f: impl FnOnce(&P) -> Q) -> Recognize<Q> {
        let (partition, order, matching) = (self.partition.clone(), self.order.clone(), self.matching.clone());
        let (measures, names) = (self.measures.clone(), self.names.clone());
        Recognize { partition, order, matching, measures, names, rows: self.rows, input: f(&self.input) }
    }

    /// Its expressions: its measures', its definitions' values', not those
    /// of its definitions' conditions, which `conds` gives.
    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        self.measures.exprs().chain(self.matching.parts().0)
    }

    /// Its definitions' conditions.
    pub fn conds(&self) -> impl Iterator<Item = &Cond> {
        self.matching.parts().1
    }

    /// Its expressions and conditions' expressions, each replaced by `f` of
    /// it.
    pub fn map_exprs(&mut self, f: &mut dyn FnMut(Expr) -> Expr) {
        self.measures.map_exprs(f);
        self.matching.map_exprs(f);
    }

    /// \[DESIGN\] Whether computing it can raise a run-time error: always,
    /// since a search may take more than `pattern::MAX_STEPS`.
    pub fn can_raise(&self) -> bool {
        true
    }

    /// Its columns' display names, its input's columns named `input`: its
    /// input's, then the measures'.
    pub fn names(&self, input: &[String]) -> Vec<String> {
        let mut out = input.to_vec();
        out.extend(self.names.iter().cloned());
        out
    }

    /// It as text, its input's columns named `names`: its partitioning,
    /// ordering, measures, rows per match, pattern and skip.
    pub fn render(&self, names: &[String]) -> String {
        let partition: Vec<String> = self.partition.iter().map(|&c| column_name(names, c)).collect();
        let rows = match self.rows {
            RowsPerMatch::One => "one row per match",
            RowsPerMatch::AllShowEmpty => "all rows per match",
            RowsPerMatch::AllOmitEmpty => "all rows per match omit empty matches",
            RowsPerMatch::AllWithUnmatched => "all rows per match with unmatched rows",
        };
        format!(
            "Recognize partition by [{}] order by [{}] measures [{}] {rows} {}",
            partition.join(", "),
            render_keys(&self.order, names),
            self.names.join(", "),
            self.matching.render()
        )
    }
}

impl Recognize<()> {
    /// The same recognition over `input`.
    pub fn over<Q>(self, input: Q) -> Recognize<Q> {
        let Recognize { partition, order, matching, measures, names, rows, input: () } = self;
        Recognize { partition, order, matching, measures, names, rows, input }
    }
}

impl WindowMatching {
    /// Its measures' expressions and its definitions' values'.
    pub fn exprs(&self) -> impl Iterator<Item = &Expr> {
        self.measures.exprs().chain(self.matching.parts().0)
    }

    /// Its definitions' conditions.
    pub fn conds(&self) -> impl Iterator<Item = &Cond> {
        self.matching.parts().1
    }

    /// Its expressions and conditions' expressions, each replaced by `f` of
    /// it.
    pub fn map_exprs(&mut self, f: &mut dyn FnMut(Expr) -> Expr) {
        self.measures.map_exprs(f);
        self.matching.map_exprs(f);
    }
}
